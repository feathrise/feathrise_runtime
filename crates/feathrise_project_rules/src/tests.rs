//! 验证无任务、无 Yarn 的独立运行，以及两位接收人、原子交付和跨存档的一次性效果。

use super::*;
use serde_json::{Value as Json, json};
use std::collections::BTreeMap;

fn book(rules: Json) -> ProjectVariables {
    ProjectVariables::from_json(
        &json!({
            "version":1, "variables":{
                "recipient":{"default":"none","choices":["none","smith","guard"]},
                "stage":{"default":0}, "known":{"default":false}
            }, "rules":rules
        })
        .to_string(),
    )
    .unwrap()
}

fn name(value: &str) -> Value {
    Value::Text(value.into())
}

#[test]
fn tagged_scene_values_cover_same_id_and_restore_independent_progress() {
    let configuration = json!({
        "version":1,
        "variables":{
            "route":{"default":"home","choices":["home","road"],"tag":"陆地"},
            "shared":{"default":false}
        },
        "scenes":{
            "MAP-port":{"variables":{
                "route":{"default":"sea","choices":["sea","deep"],"tag":"海上"},
                "crew":{"default":1}
            }},
            "MAP-cave":{"variables":{"route":{"default":"dark"},"crew":{"default":"none"}}}
        }
    }).to_string();
    let vars = ProjectVariables::from_json(&configuration).unwrap();
    assert!(vars.matches_declaration("route", &name("any-text")));
    assert!(!vars.matches_declaration("crew", &Value::Number(0.)));
    assert!(!vars.matches_declaration("unknown", &name("")));
    vars.set("route", name("road")).unwrap();
    let revision = vars.values_revision();
    vars.set_active_scene(Some("MAP-port"));
    vars.set_active_scene(Some("MAP-port"));
    assert_eq!(vars.values_revision(), revision + 1);
    assert_eq!(vars.values()["route"], name("sea"));
    assert_eq!(vars.get("crew"), Some(Value::Number(1.)));
    assert!(vars.set("route", name("road")).is_err());
    vars.set("route", name("deep")).unwrap();
    vars.set("shared", Value::Bool(true)).unwrap();
    vars.set_active_scene(Some("MAP-cave"));
    assert_eq!(vars.get("route"), Some(name("dark")));
    vars.set_active_scene(None);
    assert_eq!(vars.get("route"), Some(name("road")));
    assert_eq!(vars.get("crew"), None);
    assert_eq!(vars.get("shared"), Some(Value::Bool(true)));

    let saved = serde_json::to_value(vars.snapshot()).unwrap();
    assert_eq!(saved["values"]["route"], "road");
    assert_eq!(saved["scenes"]["MAP-port"]["values"]["route"], "deep");
    let reopened = ProjectVariables::from_json(&configuration).unwrap();
    reopened.restore(serde_json::from_value(saved).unwrap());
    assert_eq!(reopened.active_scene(), None);
    reopened.set_active_scene(Some("MAP-port"));
    assert_eq!(reopened.get("route"), Some(name("deep")));
}

#[test]
fn scene_rules_replace_the_original_slot_and_once_marks_stay_in_each_scope() {
    let vars = ProjectVariables::from_json(&json!({
        "version":1,
        "variables":{"a":{"default":0}, "route":{"default":"road"}},
        "rules":[
            {"id":"same","tag":"工程","once":true,"when":{"event":"start"},"actions":[{"request":"notes","target":"global"}]},
            {"id":"shared","when":{"event":"start"},"if":[{"variable":"a","equals":0}],"actions":[
                {"set":"route","value":"road"}, {"request":"notes","target":"shared"}]},
            {"id":"stop","when":{"event":"old"},"actions":[{"request":"notes","target":"old"}]}
        ],
        "scenes":{
            "MAP-port":{"variables":{"a":{"default":0}, "route":{"default":"sea","choices":["sea"]}}, "rules":[
                {"id":"tail","when":{"event":"start"},"if":[{"variable":"a","equals":0}],"actions":[{"request":"notes","target":"tail"}]},
                {"id":"stop","when":{"event":"new"},"actions":[{"request":"notes","target":"new"}]},
                {"id":"same","tag":"场景","once":true,"when":{"event":"start"},"actions":[
                    {"set":"a","value":1},{"request":"notes","target":"local"}]}
            ]}
        }
    }).to_string()).unwrap();
    vars.set_active_scene(Some("MAP-port"));
    vars.event("old", "");
    assert!(vars.take_requests("notes").is_empty());
    vars.event("start", "");
    // 场景版保留工程执行位，后续条件取同一份动作前状态，枚举动作失败也保留同规则的其他动作。
    assert_eq!(vars.take_requests("notes"), ["local", "shared", "tail"]);
    assert_eq!(vars.get("route"), Some(name("sea")));
    vars.set_active_scene(None);
    vars.event("start", "");
    assert_eq!(vars.take_requests("notes"), ["global", "shared"]);
    let saved = serde_json::to_value(vars.snapshot()).unwrap();
    assert_eq!(saved["fired"], json!(["same"]));
    assert_eq!(saved["scenes"]["MAP-port"]["fired"], json!(["same"]));
    vars.restore(serde_json::from_value(saved).unwrap());
    vars.set_active_scene(Some("MAP-port"));
    vars.set("a", Value::Number(0.)).unwrap();
    vars.event("start", "");
    assert_eq!(vars.take_requests("notes"), ["shared", "tail"]);
}

#[test]
fn scene_override_rejects_a_different_engine_value_type() {
    let text = r#"{"version":1,"variables":{"a":{"default":0}},"scenes":{"MAP-port":{"variables":{"a":{"default":true}}}}}"#;
    let error = ProjectVariables::from_json(text).unwrap_err();
    assert!(error.contains("MAP-port") && error.contains("变量 a"));
}

#[test]
fn value_revision_tracks_every_authoritative_write_and_restore_but_ignores_noops_and_requests() {
    let rules = book(json!([
        {"id":"chain","when":{"event":"start"},"actions":[{"set":"known","value":true}]},
        {"id":"follow","when":{"changed":"known","to":true},"actions":[{"set":"stage","value":2}]},
        {"id":"notify","when":{"event":"notice"},"actions":[{"request":"show","target":"caption"}]}
    ]));
    let reader = rules.clone();
    let initial = reader.values_revision();
    rules.set("known", Value::Bool(false)).unwrap();
    rules.event("notice", "");
    assert_eq!(rules.take_requests("show"), ["caption"]);
    assert_eq!(reader.values_revision(), initial);
    rules.event_in_instance("start", "", "dock/a");
    assert_eq!(reader.values_revision(), initial + 2);
    rules.set_if("recipient", &name("none"), name("guard"), || false).unwrap();
    assert!(rules.set("stage", Value::Bool(false)).is_err());
    assert_eq!(reader.values_revision(), initial + 2);
    rules.set_if("recipient", &name("none"), name("guard"), || true).unwrap();
    rules.set_batch_in_instance([("stage".into(), Value::Number(3.))].into(), "dock/a").unwrap();
    rules.set_batch([("stage".into(), Value::Number(4.))].into()).unwrap();
    rules.set("stage", Value::Number(5.)).unwrap();
    assert_eq!(reader.values_revision(), initial + 6);
    rules.restore(Default::default());
    assert_eq!(reader.values_revision(), initial + 7);
    assert_eq!(reader.get("stage"), Some(Value::Number(0.)));
    assert!(serde_json::to_value(rules.snapshot()).unwrap().get("values_revision").is_none());
}

#[test]
fn cinematic_requests_preserve_instance_through_variables_and_old_saves() {
    let rules=book(json!([
        {"id":"pickup","when":{"event":"pickup","target":"sword"},"actions":[{"set":"stage","value":2}]},
        {"id":"show","when":{"changed":"stage","to":2},"actions":[{"request":"cinematic.play","target":"unseal"}]}
    ]));
    rules.event_in_instance("pickup","sword","temple/east");
    let snapshot=serde_json::to_vec(&rules.snapshot()).unwrap();
    rules.restore(serde_json::from_slice(&snapshot).unwrap());
    assert_eq!(rules.take_requests_with_instances("cinematic.play"),vec![("unseal".into(),Some("temple/east".into()))]);
    rules.restore(serde_json::from_value(json!({"requests":{"cinematic.play":["before_upgrade"]}})).unwrap());
    assert_eq!(rules.take_requests_with_instances("cinematic.play"),vec![("before_upgrade".into(),None)]);
}

#[test]
fn one_handoff_selects_one_route_and_survives_slot_switch_with_an_existing_reader() {
    let rules = json!([
        {"id":"smith","when":{"changed":"recipient","from":"none","to":"smith"},"once":true,
         "actions":[{"set":"stage","value":1},{"request":"quest.start","target":"reforge"}]},
        {"id":"guard","when":{"changed":"recipient","to":"guard"},
         "actions":[{"set":"stage","value":2},{"request":"quest.start","target":"evidence"}]}
    ]);
    let game = book(rules);
    let dialogue = game.clone();
    let mut swords = 1;
    let mut deliver = |recipient: &str| {
        game.set_if("recipient", &name("none"), name(recipient), || {
            swords -= 1;
            true
        })
        .unwrap()
    };
    assert!(deliver("smith"));
    assert!(!deliver("guard"));
    assert_eq!(swords, 0);
    assert_eq!(dialogue.get("stage"), Some(Value::Number(1.0)));
    assert!(!game.set("recipient", name("smith")).unwrap());
    let slot = serde_json::to_string(&game.snapshot()).unwrap();
    assert_eq!(game.take_requests("quest.start"), ["reforge"]);
    game.restore(Default::default());
    assert_eq!(dialogue.get("recipient"), Some(name("none")));
    dialogue.set("recipient", name("guard")).unwrap();
    assert_eq!(game.take_requests("quest.start"), ["evidence"]);
    game.restore(serde_json::from_str(&slot).unwrap());
    assert_eq!(dialogue.get("recipient"), Some(name("smith")));
    assert_eq!(game.take_requests("quest.start"), ["reforge"]);
    game.set("recipient", name("none")).unwrap();
    game.set("recipient", name("smith")).unwrap();
    assert!(game.take_requests("quest.start").is_empty());
}

#[test]
fn final_variables_trigger_the_following_show_in_the_same_instance() {
    let rules=book(json!([{"id":"next","when":{"changed":"stage","to":2},"actions":[{"request":"cinematic.play","target":"after_seal"}]}]));
    rules.set_batch_in_instance([("stage".into(),Value::Number(2.0))].into(),"temple/west").unwrap();
    assert_eq!(rules.take_requests_with_instances("cinematic.play"),vec![("after_seal".into(),Some("temple/west".into()))]);
}

#[test]
fn batch_sees_all_committed_values_and_rejects_invalid_values_before_any_write() {
    let game = book(json!([{"id":"ready","when":{"changed":"stage","to":3},
        "if":[{"variable":"known","equals":true}],
        "actions":[{"request":"announce","target":"ready"}]}]));
    let values =
        |value| BTreeMap::from([("known".into(), Value::Bool(true)), ("stage".into(), value)]);
    assert!(game.set_batch(values(name("bad"))).is_err());
    assert_eq!(game.get("known"), Some(Value::Bool(false)));
    game.set_batch(values(Value::Number(3.0))).unwrap();
    game.set_batch(values(Value::Number(3.0))).unwrap();
    assert_eq!(game.take_requests("announce"), ["ready"]);
    assert!(game.set("stage", Value::Number(f32::INFINITY)).is_err());
    assert!(
        game.set_if("recipient", &name("none"), name("missing"), || panic!(
            "无效值不能扣物"
        ))
        .is_err()
    );
    assert!(
        !game
            .set_if("recipient", &name("none"), name("smith"), || false)
            .unwrap()
    );
    assert_eq!(game.get("recipient"), Some(name("none")));
}

#[test]
fn exact_and_wildcard_events_keep_author_order_and_share_pre_action_conditions() {
    let game = book(json!([
        {"id":"specific","when":{"event":"quest.completed","target":"q1"},
         "actions":[{"set":"known","value":true},{"request":"notify","target":"first"}]},
        {"id":"any","when":{"event":"quest.completed"},"if":[{"variable":"known","equals":false}],
         "actions":[{"request":"notify","target":"second"}]},
        {"id":"other","when":{"event":"quest.completed","target":"q2"},
         "actions":[{"request":"notify","target":"other"}]}
    ]));
    game.event("quest.accepted", "q1");
    assert!(game.take_requests("notify").is_empty());
    game.event("quest.completed", "q1");
    assert_eq!(game.take_requests("notify"), ["first", "second"]);
    game.event("quest.completed", "q2");
    assert_eq!(game.take_requests("notify"), ["other"]);
}

#[test]
fn cycles_stop_within_one_input_and_can_fire_on_the_next_edge() {
    let game = book(json!([
        {"id":"set","when":{"changed":"stage","to":1},"actions":[{"set":"stage","value":2},{"request":"sign","target":"tick"}]},
        {"id":"back","when":{"changed":"stage","to":2},"actions":[{"set":"stage","value":1}]}
    ]));
    for _ in 0..2 {
        game.set("stage", Value::Number(0.0)).unwrap();
        game.set("stage", Value::Number(1.0)).unwrap();
        assert_eq!(game.take_requests("sign"), ["tick"]);
        assert_eq!(game.get("stage"), Some(Value::Number(1.0)));
    }
}

#[test]
fn old_slot_defaults_and_changed_declarations_are_restored_without_new_events() {
    let game = book(
        json!([{"id":"detect","when":{"changed":"known"},"actions":[{"request":"notify","target":"unexpected"}]}]),
    );
    game.restore(
        serde_json::from_value(json!({"values":{"removed":3,"stage":"old","known":true}})).unwrap(),
    );
    assert_eq!(game.get("removed"), None);
    assert_eq!(game.get("stage"), Some(Value::Number(0.0)));
    assert_eq!(game.get("known"), Some(Value::Bool(true)));
    assert!(game.take_requests("notify").is_empty());
    game.restore(serde_json::from_str("{}").unwrap());
    assert_eq!(game.get("known"), Some(Value::Bool(false)));
}

#[test]
fn configuration_rejects_unknown_references_invalid_choices_and_misspelt_fields() {
    assert!(ProjectVariables::from_json("\u{feff}{\"version\":1}").is_ok());
    let duplicates = br#"{"version":1,"variables":{"a":{"default":1},"\u0061":{"default":2}}}"#;
    assert!(ProjectVariables::from_slice(duplicates).unwrap_err().contains("重复"));
    for (variables, rules) in [
        (json!({"a":{"default":1,"choices":["x"]}}), json!([])),
        (json!({"a":{"default":"x","choices":["x","x"]}}), json!([])),
        (
            json!({"a":{"default":false}}),
            json!([{"id":"r","when":{"changed":"a","to":1},"actions":[{"set":"a","value":true}]}]),
        ),
        (
            json!({}),
            json!([{"id":"r","when":{"event":"x","taget":"q"},"actions":[{"request":"open","target":"q"}]}]),
        ),
        (
            json!({}),
            json!([{"id":"r","when":{"event":"x"},"actions":[{"set":"absent","value":true}]}]),
        ),
        (
            json!({}),
            json!([{"id":"r","when":{"event":"x"},"actions":[]}]),
        ),
    ] {
        let doc = json!({"version":1,"variables":variables,"rules":rules});
        assert!(
            ProjectVariables::from_json(&doc.to_string()).is_err(),
            "{doc}"
        );
    }
}

#[test]
fn walking_across_a_location_fires_edges_but_loading_inside_only_sets_the_baseline() {
    let game = ProjectVariables::from_json(&json!({
        "version":1, "locations":{"gate":{"scene":"town","min":[0,0,0],"max":[3,3,3]}},
        "rules":[{"id":"enter","when":{"event":"location.entered","target":"gate"},"actions":[{"request":"marker","target":"in"}]},
                 {"id":"leave","when":{"event":"location.exited","target":"gate"},"actions":[{"request":"marker","target":"out"}]}]
    }).to_string()).unwrap();
    let mut tracker = LocationTracker::default();
    let outside = [-1.0, 1.0, 1.0];
    let inside = [1.0, 1.0, 1.0];
    tracker.observe(&game, "town", 1, outside);
    tracker.observe(&game, "town", 1, inside);
    tracker.observe(&game, "town", 1, [2.0, 1.0, 1.0]);
    tracker.observe(&game, "town", 1, outside);
    assert_eq!(game.take_requests("marker"), ["in", "out"]);
    game.restore(Default::default());
    tracker.observe(&game, "town", 1, inside);
    tracker.observe(&game, "town", 2, outside);
    tracker.observe(&game, "another", 2, inside);
    tracker.observe(&game, "town", 2, inside);
    assert!(game.take_requests("marker").is_empty());
}
