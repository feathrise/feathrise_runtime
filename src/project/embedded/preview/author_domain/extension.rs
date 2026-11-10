//! SDK只交付受控的不透明领域请求；真实模型和业务就绪由工程插件提供同身份回执。
use bevy::prelude::*;
use feathrise_scene_content::SceneId;
use std::{sync::Arc, time::Instant};

#[derive(Resource, Clone)]
pub struct AuthorDomainExtension {
    pub scene_id: SceneId,
    pub revision: u64,
    pub domain: String,
    pub payload: Arc<Vec<u8>>,
}
#[derive(Resource)]
pub struct AuthorDomainExtensionStatus {
    pub scene_id: SceneId,
    pub revision: u64,
    pub domain: String,
    pub error: Option<String>,
}
/// 游戏领域公布实际采用的显示根；只读验收从实体组件核对，不把请求或ACK当作画面完成。
#[derive(Resource, Clone, PartialEq)]
pub struct AuthorDomainExtensionScene {
    pub scene_id: SceneId,
    pub revision: u64,
    pub domain: String,
    pub identity: String,
    pub root: Option<Entity>,
    pub geometry: Option<Entity>,
    pub ready: bool,
    pub position: Option<[f32; 3]>,
    pub root_visible: bool,
    pub geometry_visible: bool,
    pub vertices: usize,
}
impl AuthorDomainExtensionScene {
    #[cfg(feature = "validation")]
    pub(crate) fn report(&self) -> serde_json::Value {
        serde_json::json!({"scene_id":self.scene_id,"domain":self.domain,"revision":self.revision,"identity":self.identity,"ready":self.ready,"root":self.root.map(Entity::to_bits),"geometry":self.geometry.map(Entity::to_bits),"position":self.position,"root_visible":self.root_visible,"geometry_visible":self.geometry_visible,"vertices":self.vertices,"source":"real-domain-scene-components"})
    }
}
#[derive(Default)]
pub(super) struct State {
    current: Option<(SceneId, u64, String, Instant)>,
}
impl State {
    pub(super) fn receive(
        &mut self,
        world: &mut World,
        value: AuthorDomainExtension,
    ) -> Result<(), String> {
        if self.current.is_some() {
            return Err("作者扩展前一完整结果尚未回执，不能被姿态请求覆盖".into());
        }
        if value.domain.is_empty()
            || value.domain.len() > 1024
            || value.domain.contains('\0')
            || value.revision == 0
        {
            return Err("作者扩展身份无效".into());
        }
        self.current = Some((
            value.scene_id.clone(),
            value.revision,
            value.domain.clone(),
            Instant::now(),
        ));
        world.remove_resource::<AuthorDomainExtensionStatus>();
        world.remove_resource::<AuthorDomainExtensionScene>();
        world.insert_resource(value);
        Ok(())
    }
    pub(super) fn advance(&mut self, world: &World) -> Result<bool, String> {
        let Some((map, revision, domain, started)) = &self.current else {
            return Ok(true);
        };
        if let Some(status) = world
            .get_resource::<AuthorDomainExtensionStatus>()
            .filter(|s| &s.scene_id == map && s.revision == *revision && &s.domain == domain)
        {
            if let Some(error) = &status.error {
                return Err(error.clone());
            }
            self.current = None;
            return Ok(true);
        }
        if started.elapsed().as_secs() > 60 {
            return Err(format!("工程未在60秒内确认作者扩展{domain}"));
        }
        Ok(false)
    }
    pub(super) fn clear(&mut self, world: &mut World) {
        self.current = None;
        world.remove_resource::<AuthorDomainExtension>();
        world.remove_resource::<AuthorDomainExtensionStatus>();
        world.remove_resource::<AuthorDomainExtensionScene>();
    }
    #[cfg(feature = "validation")]
    pub(super) fn evidence(&self, world: &World) -> serde_json::Value {
        let Some(request) = world.get_resource::<AuthorDomainExtension>() else {
            return serde_json::Value::Null;
        };
        let matches = |map: &SceneId, revision: u64, domain: &str| {
            map == &request.scene_id && revision == request.revision && domain == request.domain
        };
        let status = world
            .get_resource::<AuthorDomainExtensionStatus>()
            .filter(|s| matches(&s.scene_id, s.revision, &s.domain));
        let scene=world.get_resource::<AuthorDomainExtensionScene>().filter(|s|matches(&s.scene_id,s.revision,&s.domain)).map(|s| {
            let root_visible=s.root.and_then(|root|world.get::<Visibility>(root)).is_some_and(|v|*v!=Visibility::Hidden);
            let geometry_visible=s.geometry.and_then(|root|world.get::<Visibility>(root)).is_some_and(|v|*v!=Visibility::Hidden);
            let vertices=s.geometry.and_then(|e|world.get::<Mesh3d>(e)).and_then(|mesh|world.get_resource::<Assets<Mesh>>()?.get(&mesh.0)).map(Mesh::count_vertices);
            serde_json::json!({"identity":s.identity,"ready":s.ready,"root":s.root.map(Entity::to_bits),"geometry":s.geometry.map(Entity::to_bits),"root_exists":s.root.is_some_and(|e|world.get_entity(e).is_ok()),"root_visible":root_visible,"geometry_visible":geometry_visible,"vertices":vertices,"position":s.root.and_then(|e|world.get::<Transform>(e)).map(|t|t.translation.to_array())})
        });
        serde_json::json!({"scene_id":request.scene_id,"domain":request.domain,"revision":request.revision,"acknowledged":status.is_some_and(|s|s.error.is_none()),"error":status.and_then(|s|s.error.as_ref()),"scene":scene})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opaque_content_keeps_request_until_exact_ready_before_pose_can_enter() {
        let map = SceneId::parse("MAP-external-preview").unwrap();
        let mut world = World::new();
        let mut state = State::default();
        let content = AuthorDomainExtension {
            scene_id: map.clone(),
            revision: 4,
            domain: "test/content".into(),
            payload: Arc::new(vec![1, 2]),
        };
        state.receive(&mut world, content.clone()).unwrap();
        let pose = AuthorDomainExtension {
            scene_id: map.clone(),
            revision: 5,
            domain: "test/pose".into(),
            payload: Arc::new(vec![3]),
        };
        assert!(state.receive(&mut world, pose.clone()).is_err());
        assert_eq!(
            world.resource::<AuthorDomainExtension>().domain,
            "test/content"
        );
        world.insert_resource(AuthorDomainExtensionStatus {
            scene_id: map.clone(),
            revision: 5,
            domain: "test/pose".into(),
            error: None,
        });
        assert!(
            !state.advance(&world).unwrap(),
            "旧/其他槽回执不能释放内容事务"
        );
        world.insert_resource(AuthorDomainExtensionStatus {
            scene_id: map.clone(),
            revision: 4,
            domain: "test/content".into(),
            error: None,
        });
        assert!(state.advance(&world).unwrap());
        state.receive(&mut world, pose).unwrap();
        assert_eq!(world.resource::<AuthorDomainExtension>().revision, 5);
        state.clear(&mut world);
        assert!(!world.contains_resource::<AuthorDomainExtension>());
    }
}
