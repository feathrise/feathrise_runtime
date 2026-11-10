//! 原作者局部结果进入现有正式资源；后台处理大数据，主线程只采用完整身份和固定小网格。
mod height;
mod mesh;
mod object_pose;
#[cfg(test)]
mod tests;
mod water;
pub(crate) mod extension;
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use feathrise_scene_content::{
    SceneId,
    embedded_author_preview::{AUTHOR_PREVIEW_FORMAT, AuthorPreview, decode},
};

pub(super) struct Prepared {
    value: Value,
}
enum Value {
    Extension(extension::AuthorDomainExtension),
    Mesh(mesh::Prepared),
    Height(height::Prepared),
    Water(water::Prepared),
    ObjectPose(object_pose::Prepared),
}
pub(super) fn prepare(format: &str, bytes: &[u8], map: &SceneId) -> Result<Prepared, String> {
    if format != AUTHOR_PREVIEW_FORMAT {
        return Err("作者局部预览格式不兼容".into());
    }
    let value = decode(bytes)?;
    if value.scene_id() != map {
        return Err("作者局部预览正文场景与控制身份不一致".into());
    }
    let value = match value {
        AuthorPreview::Extension {scene_id,revision,domain,payload}=>Value::Extension(extension::AuthorDomainExtension {scene_id:scene_id,revision,domain,payload}),
        AuthorPreview::Mesh {
            scene_id,
            revision,
            value,
            committed,
        } => Value::Mesh(mesh::prepare(scene_id, revision, value, committed)?),
        AuthorPreview::Height {
            scene_id,
            terrain_id,
            revision,
            reset,
            tiles,
        } => Value::Height(height::prepare(scene_id, terrain_id, revision, reset, tiles)?),
        AuthorPreview::Water {
            scene_id,
            terrain_id,
            root,
            revision,
            definition,
            masks,
        } => Value::Water(water::prepare(
            scene_id, terrain_id, root, revision, definition, masks,
        )?),
        AuthorPreview::SubscenePose { scene_id, revision, id, pose, active } =>
            Value::ObjectPose(object_pose::Prepared { scene_id:scene_id, revision, active,
                target:object_pose::Target::Subscene { id,pose } }),
        AuthorPreview::TerrainPose { scene_id, revision, id, pose, active } =>
            Value::ObjectPose(object_pose::Prepared { scene_id:scene_id, revision, active,
                target:object_pose::Target::Terrain { id,pose } }),
    };
    Ok(Prepared { value })
}
pub(super) fn install(app: &mut App) {
    mesh::install(app);
}
#[derive(Default)]
pub(super) struct State {
    extension:extension::State,
    pending: Option<Value>,
    height: height::State,
    mesh: mesh::State,
    water: water::State,
    object_pose: object_pose::State,
    garbage: Option<Task<()>>,
    #[cfg(feature = "validation")]
    last: Option<&'static str>,
}
impl State {
    pub(super) fn clear_water(&mut self, world: &mut World) -> Result<bool, String> {
        self.water.clear_published(world)
    }
    pub(super) fn receive(&mut self, _: &mut World, value: Prepared) -> Result<(), String> {
        if self.pending.is_some() || self.height.busy() || self.garbage.is_some() {
            return Err("作者局部预览超过单飞预算".into());
        }
        #[cfg(feature = "validation")]
        {
            self.last = Some(match &value.value {
                Value::Mesh(_) => "mesh",
                Value::Height(_) => "height",
                Value::Water(_) => "water",
                Value::ObjectPose(_) => "object-pose",
                Value::Extension(_) => "extension",
            });
        }
        self.pending = Some(value.value);
        Ok(())
    }
    pub(super) fn advance(&mut self, world: &mut World, sources_settled: bool, sources_ready: bool) -> Result<bool, String> {
        self.mesh.reconcile(world);
        if !self.extension.advance(world)? {return Ok(false);}
        if !self.object_pose.advance(world)? { return Ok(false); }
        if let Some(task) = self.garbage.as_mut() {
            if check_ready(task).is_none() {
                return Ok(false);
            }
            self.garbage = None;
        }
        if self.height.busy() {
            let Some(old) = self.height.advance(world)? else {
                return Ok(false);
            };
            self.garbage = Some(AsyncComputeTaskPool::get().spawn(async move {
                drop(old);
            }));
            return Ok(false);
        }
        // SnapshotApplied只表示内存来源已发布；新ID必须等本次真实loader采用后才查正式清单。
        // 保留当前唯一正文，不等待尚未发送的未来快照，也不阻断其它作者域的现有实时路径。
        if matches!(self.pending, Some(Value::ObjectPose(_))) {
            if !sources_settled { return Ok(false); }
            if !sources_ready { return Err("整体姿态所依赖的已发布场景来源加载失败".into()); }
        }
        let Some(pending) = self.pending.take() else {
            return Ok(true);
        };
        match pending {
            Value::Extension(value)=>{self.extension.receive(world,value)?;return Ok(false);}
            Value::Mesh(value) => self.mesh.receive(world, value)?,
            Value::Height(value) => {
                self.height.start(world, value)?;
                return Ok(false);
            }
            Value::Water(mut value) => {
                if !self.water.advance(world, &mut value)? {
                    self.pending = Some(Value::Water(value));
                    return Ok(false);
                }
                self.garbage = Some(AsyncComputeTaskPool::get().spawn(async move {
                    drop(value);
                }));
                return Ok(false);
            }
            Value::ObjectPose(value) => {
                self.object_pose.start(world,value)?;
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub(super) fn snapshot_started(&mut self, world: &mut World) -> Result<(), String> {
        self.extension.clear(world);
        self.object_pose.snapshot_started()?;
        if self.pending.is_some() || self.height.busy() || self.garbage.is_some() {
            return Err("完整快照不能越过尚未完成的作者局部预览".into());
        }
        // 正式候选尚未GPU就绪；保留提交幽灵直到实际活动根接管，不能把收到快照当成显示完成。
        self.mesh.handoff();
        Ok(())
    }
    #[cfg(feature = "validation")]
    pub(super) fn evidence(&self, world: &World) -> serde_json::Value {
        serde_json::json!({"domain":self.last,"mesh":self.mesh.evidence(world),"height":self.height.evidence(world),"water":self.water.evidence(world),"object_pose":self.object_pose.evidence(world),"extension":self.extension.evidence(world)})
    }
}
