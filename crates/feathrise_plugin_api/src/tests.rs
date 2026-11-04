//! 纯ABI回归：坏消息、panic隔离、销毁责任和快照资源边界。
use super::*;
use std::ffi::c_void;
struct PanicTool;
impl NativeTool for PanicTool {
    fn create(_: SessionConfig) -> Result<Self, String> {
        Ok(Self)
    }
    fn request(&mut self, _: Request) -> Snapshot {
        panic!("test panic boundary")
    }
}
unsafe extern "C" fn sink(_: *mut c_void, _: *const u8, _: usize) -> i32 {
    OK
}
#[test]
fn invalid_input_and_panicking_session_cannot_reenter() {
    let config = serde_json::to_vec(&SessionConfig {
        plugin_id: "test".into(),
        project_id: "test".into(),
        project_root: ".".into(),
        asset_root: ".".into(),
    })
    .unwrap();
    let mut handle = std::ptr::null_mut();
    unsafe {
        assert_eq!(
            create::<PanicTool>(std::ptr::null(), 0, &mut handle, std::ptr::null_mut(), sink),
            INVALID
        );
        assert!(handle.is_null());
        assert_eq!(
            create::<PanicTool>(
                config.as_ptr(),
                config.len(),
                &mut handle,
                std::ptr::null_mut(),
                sink
            ),
            OK
        );
        assert_eq!(
            invoke::<PanicTool>(
                handle,
                config.as_ptr(),
                MAX_MESSAGE_BYTES + 1,
                std::ptr::null_mut(),
                sink
            ),
            INVALID
        );
        let request = serde_json::to_vec(&Request::Open).unwrap();
        assert_eq!(
            invoke::<PanicTool>(
                handle,
                request.as_ptr(),
                request.len(),
                std::ptr::null_mut(),
                sink
            ),
            FAULT
        );
        assert_eq!(
            invoke::<PanicTool>(
                handle,
                request.as_ptr(),
                request.len(),
                std::ptr::null_mut(),
                sink
            ),
            FAULT
        );
        destroy::<PanicTool>(handle);
    }
}
#[test]
fn snapshot_rejects_foreign_identity_escape_paths_and_fractional_numbers() {
    let mut snapshot = Snapshot {
        plugin_id: "test".into(),
        tool: "工具".into(),
        ..Default::default()
    };
    assert!(snapshot.validate("test").is_ok());
    assert!(snapshot.validate("other").is_err());
    snapshot.preview = Some(Preview {
        revision: 1,
        model: Some("../other/model.glb".into()),
        icon: None,
        details: String::new(),
        runtime: None,
    });
    assert!(snapshot.validate("test").is_err());
    snapshot.preview = None;
    snapshot.fields.push(Field {
        id: "x".into(),
        label: "X".into(),
        value: "1".into(),
        kind: FieldKind::Number {
            min: 0.,
            max: 5.,
            step: 0.1,
        },
    });
    assert!(snapshot.validate("test").is_err());
}

#[test]
fn workbench_rejects_dangling_fields_and_invalid_catalog_windows() {
    let mut s = Snapshot {
        plugin_id: "test".into(),
        tool: "工具".into(),
        workbench: Some(Workbench {
            groups: vec![FieldGroup {
                title: "分组".into(),
                help: String::new(),
                fields: vec!["missing".into()],
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(s.validate("test").is_err());
    s.workbench.as_mut().unwrap().groups.clear();
    s.workbench.as_mut().unwrap().catalog = Some(CatalogView {
        title: "目录".into(),
        help: String::new(),
        action: "choose".into(),
        total: 1000,
        offset: 1,
        columns: 6,
    });
    assert!(s.validate("test").is_err());
    s.workbench
        .as_mut()
        .unwrap()
        .catalog
        .as_mut()
        .unwrap()
        .offset = 0;
    assert!(s.validate("test").is_ok());
    s.workbench
        .as_mut()
        .unwrap()
        .catalog
        .as_mut()
        .unwrap()
        .columns = 0;
    assert!(s.validate("test").is_err());
}

#[test]
fn decimal_timeline_and_runtime_preview_keep_exact_values_and_identity() {
    let mut s=Snapshot{plugin_id:"fixture".into(),tool:"技能".into(),..Default::default()};
    s.fields.push(Field{id:"offset".into(),label:"位移".into(),value:"-0.125".into(),
        kind:FieldKind::Decimal{min:-10.,max:10.,step:0.001,precision:3}});
    s.timeline=Some(Timeline{duration:2.,cursor:0.5,playing:false,zoom:1.,pan:0.,span:None,lanes:vec![TimelineLane{
        id:"motion".into(),label:"动作".into(),presentation:Default::default(),blocks:vec![TimelineBlock{id:"7".into(),label:"斩击".into(),start:0.25,end:1.5,selected:true,presentation:Default::default()}]}]});
    s.preview=Some(Preview{revision:1,model:None,icon:None,details:String::new(),runtime:Some(RuntimePreview{
        revision:1,kind:"fixture.skill".into(),payload:serde_json::json!({"offset":-0.125})})});
    assert!(s.validate("fixture").is_ok());
    let roundtrip:Snapshot=serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
    assert_eq!(roundtrip.fields[0].value,"-0.125");
    s.timeline.as_mut().unwrap().lanes[0].blocks[0].end=3.;assert!(s.validate("fixture").is_err());
    s.timeline.as_mut().unwrap().lanes[0].blocks[0].end=1.;
    s.preview.as_mut().unwrap().runtime.as_mut().unwrap().revision=0;assert!(s.validate("fixture").is_err());
}

#[test]
fn timeline_legacy_defaults_and_explicit_reference_presentation_are_compatible() {
    let mut axis: Timeline = serde_json::from_value(serde_json::json!({
        "duration":2.,"cursor":0.,"playing":false,"zoom":1.,"pan":0.,
        "lanes":[{"id":"lane","label":"轨道","blocks":[{
            "id":"main","label":"区间","start":0.25,"end":1.5,"selected":true
        }]}]
    })).unwrap();
    let encoded = serde_json::to_value(&axis).unwrap();
    assert!(encoded.get("span").is_none());
    assert!(encoded["lanes"][0].get("presentation").is_none());
    assert!(encoded["lanes"][0]["blocks"][0].get("presentation").is_none());
    assert!(axis.lanes[0].blocks[0].presentation.resize_start);
    assert!(axis.lanes[0].blocks[0].presentation.resize_end);
    axis.span = Some(0.25);
    axis.lanes[0].presentation.color = Some([0.18,0.48,0.52,0.88]);
    axis.lanes[0].blocks.push(TimelineBlock {
        id:"reference".into(),label:"另一个时序".into(),start:0.5,end:3.,selected:false,
        presentation:TimelineBlockPresentation {read_only:true,reference_of:Some("main".into()),
            reference_band:1,..Default::default()},
    });
    let mut snapshot = Snapshot { plugin_id:"fixture".into(), tool:"时间轴".into(),
        timeline:Some(axis), ..Default::default() };
    assert!(snapshot.validate("fixture").is_ok(), "参考条可超出主轴，且不改变主轴跨度");
    snapshot.timeline.as_mut().unwrap().lanes[0].blocks[1].presentation.read_only = false;
    assert!(snapshot.validate("fixture").is_err(), "参考关系不能保留第二个编辑入口");
    snapshot.timeline.as_mut().unwrap().lanes[0].blocks[1].presentation.read_only = true;
    snapshot.timeline.as_mut().unwrap().span = Some(f64::NAN);
    assert!(snapshot.validate("fixture").is_err());
    snapshot.timeline.as_mut().unwrap().span = Some(0.25);
    snapshot.timeline.as_mut().unwrap().lanes[0].presentation.color = Some([1.1,0.,0.,1.]);
    assert!(snapshot.validate("fixture").is_err());
}

#[test]
fn timeline_validates_derived_span_and_window_end_without_rejecting_finite_boundaries() {
    let tiny=f64::from_bits(1);
    for (duration,zoom,pan,span,valid) in [
        (f64::MAX,0.1,0.,None,false),
        (tiny,100.,0.,None,false),
        (f64::MAX,1.,f64::MAX,Some(f64::MAX),false),
        (1.,1.,0.,Some(0.),false),
        (1.,1.,0.,Some(f64::NAN),false),
        (f64::MAX,1.,0.,None,true),
        (1.,100.,0.,Some(tiny),true),
        (f64::MAX,1.,f64::MAX,Some(tiny),true),
        (1.,1.,0.,None,true),
    ] {
        let snapshot=Snapshot {plugin_id:"fixture".into(),tool:"时间轴".into(),
            timeline:Some(Timeline {duration,cursor:0.,playing:false,zoom,pan,span,lanes:vec![]}),
            ..Default::default()};
        assert_eq!(snapshot.validate("fixture").is_ok(),valid,
            "duration={duration:?} zoom={zoom:?} pan={pan:?} span={span:?}");
    }
}
