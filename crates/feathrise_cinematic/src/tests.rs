//! 反向拖帧与固定帧率导出必须和从头播放得到同一机位、动作相位及字幕。
use super::*;

#[test]
fn recurring_cinematic_keeps_only_the_latest_pose_per_instance_in_completion_order() {
    let document = std::sync::Arc::new(Cinematic::new("sword", "拔剑"));
    let mut state = CinematicWorldState::default();
    for (ticket, instance) in [(1, "dock/a"), (2, "dock/b"), (3, "dock/a")] {
        state.record_completion(FrameData {
            document: document.clone(),
            time: document.duration,
            ticket,
            instance: Some(instance.into()),
            mode: FrameMode::Game,
        });
    }
    let tickets: Vec<_> = state.completed.iter().map(|frame| frame.ticket).collect();
    assert_eq!(tickets, [2, 3]);
}

#[test]
fn film_holds_world_time_and_uses_only_the_chosen_yarn_transcript() {
    use movie::{MovieSettings, MovieTimeline};
    let doc = Cinematic::from_slice(
        br#"{"version":1,"id":"f","name":"film","duration":2,
      "dialogue":[{"id":"talk","time":0.5,"duration":0.25,"yarn":"Route","wait":true},
      {"id":"caption","time":1,"duration":0.5,"text":"Later"}]}"#,
    )
    .unwrap();
    let options = MovieSettings {
        fps: 4,
        seconds_per_line: 0.5,
        ..Default::default()
    };
    assert!(MovieTimeline::new(&doc, &options, &Default::default()).is_err());
    let t = MovieTimeline::new(
        &doc,
        &options,
        &[("talk".into(), vec!["First".into(), "Selected".into()])].into(),
    )
    .unwrap();
    assert_eq!(t.frames, 12);
    assert_eq!(
        t.frame(2).unwrap(),
        movie::MovieFrame {
            time: 0.5,
            subtitles: vec!["First"]
        }
    );
    assert_eq!(
        t.frame(5).unwrap(),
        movie::MovieFrame {
            time: 0.5,
            subtitles: vec!["Selected"]
        }
    );
    assert_eq!(
        t.frame(8).unwrap(),
        movie::MovieFrame {
            time: 1.,
            subtitles: vec!["Later"]
        }
    );
    assert!(t.frame(12).is_none());
    assert!(
        MovieSettings {
            width: 641,
            ..options
        }
        .validate()
        .is_err()
    );
}

#[test]
fn arbitrary_seek_keeps_cuts_motion_phase_and_dialogue_in_sync() {
    let file=br#"{"version":1,"id":"sword","name":"sword","duration":4,
        "camera":[{"time":0,"pose":{}},{"time":2,"pose":{"translation":[4,0,0]},"smooth":true},{"time":3,"pose":{"translation":[9,0,0]}}],
        "actors":[{"target":"player","keys":[{"time":0,"pose":{}},{"time":2,"pose":{"translation":[0,0,4]}}],
        "clips":[{"time":1,"duration":2,"name":"draw"}]}],
        "dialogue":[{"id":"line","time":1,"duration":1,"text":"Sword","wait":true}]}"#;
    let doc = Cinematic::from_slice(file).unwrap();
    assert_eq!(doc.sample(4.).camera.unwrap().pose.translation[0], 9.);
    let midway = doc.sample(1.);
    assert_eq!(midway.camera.unwrap().pose.translation[0], 2.);
    assert_eq!(midway.actors[0].pose.unwrap().translation[2], 2.);
    assert_eq!(midway.actors[0].clip.unwrap().time, 0.);
    assert_eq!(midway.dialogue[0].id, "line");
    assert!(doc.sample(2.).dialogue.is_empty());
    assert_eq!(doc.sample(2.9).camera.unwrap().pose.translation[0], 4.);
    assert_eq!(doc.sample(4.).actors[0].clip.unwrap().time, 2.);
    assert!(CinematicCatalog::from_documents([doc.clone(), doc]).is_err());
}

#[test]
fn invalid_pose_and_two_bindings_with_the_same_identity_are_diagnosed() {
    let mut doc = Cinematic::new("foo", "Foo");
    let mut key = CameraKey {
        time: 0.,
        pose: Pose::default(),
        fov_y_degrees: 45.,
        smooth: false,
    };
    key.pose.rotation = [0.; 4];
    doc.camera.push(key);
    assert!(doc.validate().is_err());
    doc.camera.clear();
    let target = MotionTrack {
        target: "seal".into(),
        instance: Some("templeA".into()),
        keys: vec![],
        clips: vec![],
    };
    doc.objects = vec![target.clone(), target];
    assert!(doc.validate().is_err());
    doc.objects[1].instance = Some("templeB".into());
    doc.validate().unwrap();
}
