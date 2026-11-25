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
