//! 采用作者原有地表辅助纹理；固定行块更新，复用正式地形几何或Clipmap原覆盖通道。
use bevy::{asset::RenderAssetUsages,ecs::system::SystemState,image::{ImageAddressMode,ImageFilterMode,ImageSampler,ImageSamplerDescriptor},
    light::NotShadowCaster,prelude::*,render::render_resource::{Extent3d,TextureDimension,TextureFormat}};
use feathrise_editor_protocol::Mode;
use feathrise_map_content::{MapId,MapTerrainKind,embedded_surface_trail::*};
use feathrise_scene::{TerrainObjectQueries,DynamicClipmapSurfaceOverlay,ClipmapSurfaceCoverage,ClipmapSurfaceOverlayIdentity};

pub(super) struct Prepared { patch:SurfaceTrailPatch, image:Option<Image>, transform:Transform }
pub(super) fn prepare(format:&str,bytes:&[u8],map:&MapId)->Result<Prepared,String> {
    if format!=SURFACE_TRAIL_FORMAT {return Err("地表辅助预览格式不兼容".into())}
    let patch=decode_surface_trail(bytes)?;
    if &patch.map_id!=map {return Err("地表辅助正文与会话地图不符".into())}
    let transform=if patch.visible {
        // 来源是原工具已经采用的Transform，而不是等待另一预览域追平的运行时姿态。
        let matrix=Mat4::from_cols_array(&patch.source_to_world);
        let determinant=matrix.determinant();
        if !determinant.is_finite() || determinant.abs()<1e-12 || !matrix.row(3).abs_diff_eq(Vec4::W,0.000001) {return Err("地表辅助姿态不是可逆仿射变换".into())}
        let transform=Transform::from_matrix(matrix);
        if !transform.to_matrix().abs_diff_eq(matrix,0.0005) {return Err("地表辅助姿态不是原工具可表达的变换".into())}
        transform
    } else {Transform::IDENTITY};
    let image=if patch.visible&&patch.reset {
        // 新代的完整零图只在后台创建；帧内仅登记句柄，之后每次最多改16KiB。
        let mut image=Image::new_fill(Extent3d{width:SURFACE_TRAIL_DIM as u32,height:SURFACE_TRAIL_DIM as u32,depth_or_array_layers:1},
            TextureDimension::D2,&[0;4],TextureFormat::Rgba8UnormSrgb,RenderAssetUsages::default());
        image.sampler=ImageSampler::Descriptor(ImageSamplerDescriptor{address_mode_u:ImageAddressMode::ClampToEdge,
            address_mode_v:ImageAddressMode::ClampToEdge,mag_filter:ImageFilterMode::Linear,min_filter:ImageFilterMode::Linear,..default()});
        let offset=usize::from(patch.block)*SURFACE_TRAIL_BLOCK_BYTES;
        image.data.as_mut().unwrap()[offset..offset+patch.rgba.len()].copy_from_slice(&patch.rgba);
        Some(image)
    } else {None};
    Ok(Prepared{patch,image,transform})
}

#[derive(Default)]
pub(super) struct State {
    pending:Option<Prepared>,current:Option<SurfaceTrailPatch>,image:Option<Handle<Image>>,
    material:Option<Handle<StandardMaterial>>,entity:Option<Entity>,
    queries:Option<SystemState<TerrainObjectQueries<'static,'static>>>,
}
impl State {
    pub(super) fn receive(&mut self,_world:&mut World,value:Prepared)->Result<(),String> {
        if self.pending.is_some() {return Err("地表辅助超过单飞预算".into())}
        if !value.patch.reset && self.current.as_ref().is_none_or(|old|!same_source(old,&value.patch)) {
            return Err("地表辅助行块没有同代基础图".into())
        }
        self.pending=Some(value);Ok(())
    }
    pub(super) fn advance(&mut self,world:&mut World)->Result<bool,String> {
        let editing=world.resource::<super::super::EmbeddedProjectState>().mode==Mode::Editing;
        if !editing {
            if let Some(entity)=self.entity {if let Some(mut visibility)=world.get_mut::<Visibility>(entity){*visibility=Visibility::Hidden;}}
            self.clear_clipmap(world);
        }
        let Some(pending)=self.pending.as_ref() else {return Ok(true)};
        if !pending.patch.visible {self.clear(world);self.pending=None;return Ok(true)}
        if !editing {return Ok(false)}
        let query=self.queries.get_or_insert_with(||SystemState::new(world));
        let meshes=query.get(world).map_err(|error|format!("地表辅助真实地形查询失败：{error}"))?;
        let source=if let Some(id)=pending.patch.terrain_id.as_ref() {meshes.render_mesh(&pending.patch.map_id,id)}else{meshes.root_render_mesh(&pending.patch.map_id)};
        self.adopt_source(world,source)
    }
    fn adopt_source(&mut self,world:&mut World,source:Option<(Handle<Mesh>,Mat4,MapTerrainKind)>)->Result<bool,String> {
        let Some((mesh,_runtime_matrix,kind))=source else {return Ok(false)};
        let pending=self.pending.as_ref().ok_or("地表辅助缺少待采用行块")?;
        if kind!=pending.patch.terrain_kind {return Ok(false)}
        // 只借用稳定目标的真实几何；原作者已把live pose冻结在正文中。若等待运行时矩阵追平，
        // 当前行块会占住唯一正文槽，反过来挡住使其追平的姿态消息。实际采用原矩阵后才回ACK。
        let Prepared{mut patch,image,transform}=self.pending.take().unwrap();
        if patch.reset {self.clear(world);self.image=Some(world.resource_mut::<Assets<Image>>().add(image.ok_or("地表辅助新代缺少后台完整图")?));}
        else {
            let handle=self.image.as_ref().ok_or("地表辅助当前图已丢失")?;
            let mut images=world.resource_mut::<Assets<Image>>();let mut image=images.get_mut(handle).ok_or("地表辅助图资产已退役")?;
            let data=image.data.as_mut().ok_or("地表辅助图缺少CPU来源")?;
            let offset=usize::from(patch.block)*SURFACE_TRAIL_BLOCK_BYTES;
            data.get_mut(offset..offset+patch.rgba.len()).ok_or("地表辅助图行块越界")?.copy_from_slice(&patch.rgba);
        }
        let image=self.image.as_ref().unwrap().clone();
        if kind==MapTerrainKind::SparseClipmap {
            let identity=identity(&patch);
            let mut overlay=world.resource_mut::<DynamicClipmapSurfaceOverlay>();
            overlay.current=Some(identity.clone());overlay.trail=Some(ClipmapSurfaceCoverage{identity,image});
        } else {
            let material=self.material.get_or_insert_with(||world.resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial{
                base_color:Color::WHITE,base_color_texture:Some(image),alpha_mode:AlphaMode::Blend,unlit:true,depth_bias:4.,..default()})).clone();
            if let Some(entity)=self.entity {world.entity_mut(entity).insert((Mesh3d(mesh),transform,Visibility::Inherited));}
            else {self.entity=Some(world.spawn((Name::new("工程视口原地表柔边覆盖"),Mesh3d(mesh),MeshMaterial3d(material),
                    transform,Visibility::Inherited,NotShadowCaster)).id());}
        }
        patch.rgba.clear();self.current=Some(patch);Ok(true)
    }
    fn clear_clipmap(&self,world:&mut World) {
        if let Some(current)=&self.current && let Some(mut overlay)=world.get_resource_mut::<DynamicClipmapSurfaceOverlay>() {
            let own=identity(current);
            if overlay.trail.as_ref().is_some_and(|value|value.identity==own) {overlay.trail=None;}
            if overlay.current.as_ref()==Some(&own)&&overlay.selection.is_none() {overlay.current=None;}
        }
    }
    fn clear(&mut self,world:&mut World) {
        self.clear_clipmap(world);
        if let Some(entity)=self.entity.take(){let _=world.despawn(entity);}
        self.current=None;self.image=None;self.material=None;
    }
    pub(super) fn snapshot_started(&mut self,world:&mut World)->Result<(),String> {
        if self.pending.is_some(){return Err("完整快照不能越过尚未采用的地表辅助行块".into())}
        self.clear(world);Ok(())
    }
    #[cfg(feature="validation")]
    pub(super) fn evidence(&self,world:&World)->serde_json::Value {
        serde_json::json!({"source":"original-surface-stroke-trail","current":self.current,
            "entity":self.entity.map(Entity::to_bits),"actual_visible":self.entity.and_then(|entity|world.get::<Visibility>(entity)).map(|value|*value!=Visibility::Hidden),
            "texture":self.image.as_ref().map(|handle|format!("{:?}",handle.id())),
            "image_size":self.image.as_ref().and_then(|handle|world.resource::<Assets<Image>>().get(handle)).map(|image|[image.width(),image.height()])})
    }
}
fn same_source(a:&SurfaceTrailPatch,b:&SurfaceTrailPatch)->bool {
    a.source_epoch==b.source_epoch&&a.map_id==b.map_id&&a.terrain_id==b.terrain_id
        &&a.terrain_revision==b.terrain_revision&&a.source_to_world==b.source_to_world&&a.terrain_kind==b.terrain_kind
}
fn identity(patch:&SurfaceTrailPatch)->ClipmapSurfaceOverlayIdentity {
    ClipmapSurfaceOverlayIdentity{map_id:patch.map_id.clone(),terrain_id:patch.terrain_id.clone(),author_revision:patch.author_revision,
        terrain_revision:patch.terrain_revision,surface_revision:patch.surface_revision,source_to_world:Mat4::from_cols_array(&patch.source_to_world)}
}

#[cfg(test)] mod tests {
    use super::*;
    fn patch()->SurfaceTrailPatch {SurfaceTrailPatch{map_id:MapId::parse("M1-TEST").unwrap(),terrain_id:None,terrain_revision:1,
        author_revision:1,surface_revision:1,source_epoch:1,source_to_world:Mat4::IDENTITY.to_cols_array(),terrain_kind:MapTerrainKind::Heightfield,
        visible:true,reset:true,block:2,block_revision:1,rgba:vec![17;SURFACE_TRAIL_BLOCK_BYTES]}}
    #[test] fn surface_trail_preparation_preserves_original_rgba_and_rejects_unbased_blocks() {
        let source=patch();let prepared=prepare(SURFACE_TRAIL_FORMAT,&encode_surface_trail(&source).unwrap(),&source.map_id).unwrap();
        let data=prepared.image.as_ref().unwrap().data.as_ref().unwrap();assert!(data[..2*SURFACE_TRAIL_BLOCK_BYTES].iter().all(|value|*value==0));
        assert_eq!(&data[2*SURFACE_TRAIL_BLOCK_BYTES..3*SURFACE_TRAIL_BLOCK_BYTES],source.rgba.as_slice());
        let mut next=source.clone();next.reset=false;let mut state=State::default();
        assert!(state.receive(&mut World::new(),prepare(SURFACE_TRAIL_FORMAT,&encode_surface_trail(&next).unwrap(),&next.map_id).unwrap()).is_err());
        next.source_epoch+=1;assert!(!same_source(&source,&next));
    }
    #[test] fn surface_trail_adopts_original_pose_without_waiting_for_the_blocked_pose_domain() {
        let mut world=World::new();
        world.init_resource::<Assets<Mesh>>();world.init_resource::<Assets<Image>>();world.init_resource::<Assets<StandardMaterial>>();
        world.insert_resource(super::super::super::EmbeddedProjectState{mode:Mode::Editing,world_id:1,revision:1,transitioning:false});
        let mesh=world.resource_mut::<Assets<Mesh>>().add(Cuboid::new(2.,1.,3.));
        let original=Transform::from_xyz(8.,2.,-3.).with_rotation(Quat::from_rotation_y(0.3)).with_scale(Vec3::new(2.,1.,3.));
        let mut source=patch();source.source_to_world=original.to_matrix().to_cols_array();
        let mut state=State::default();
        state.receive(&mut world,prepare(SURFACE_TRAIL_FORMAT,&encode_surface_trail(&source).unwrap(),&source.map_id).unwrap()).unwrap();
        assert!(!state.adopt_source(&mut world,None).unwrap(),"没有实际地形仍不能回ACK");
        assert!(!state.adopt_source(&mut world,Some((mesh.clone(),Mat4::IDENTITY,MapTerrainKind::SparseClipmap))).unwrap(),"真实几何类型仍须匹配");
        assert!(state.adopt_source(&mut world,Some((mesh.clone(),Mat4::IDENTITY,MapTerrainKind::Heightfield))).unwrap());
        let entity=state.entity.unwrap();
        assert_eq!(world.get::<Mesh3d>(entity).unwrap().0.id(),mesh.id());
        assert!(world.get::<Transform>(entity).unwrap().to_matrix().abs_diff_eq(original.to_matrix(),0.0005));
        let image=world.resource::<Assets<Image>>().get(state.image.as_ref().unwrap()).unwrap();
        assert_eq!(&image.data.as_ref().unwrap()[2*SURFACE_TRAIL_BLOCK_BYTES..3*SURFACE_TRAIL_BLOCK_BYTES],source.rgba.as_slice());
        source.visible=false;source.rgba.clear();
        state.receive(&mut world,prepare(SURFACE_TRAIL_FORMAT,&encode_surface_trail(&source).unwrap(),&source.map_id).unwrap()).unwrap();
        assert!(state.advance(&mut world).unwrap());
        assert!(world.get_entity(entity).is_err());assert!(state.current.is_none());assert!(state.image.is_none());
    }
    #[test] fn surface_trail_clipmap_uses_original_overlay_identity_even_before_pose_catches_up() {
        let mut world=World::new();world.init_resource::<Assets<Image>>();world.init_resource::<DynamicClipmapSurfaceOverlay>();
        let mut source=patch();source.terrain_kind=MapTerrainKind::SparseClipmap;
        source.source_to_world=Transform::from_xyz(5.,0.,7.).to_matrix().to_cols_array();
        let mut state=State::default();
        state.receive(&mut world,prepare(SURFACE_TRAIL_FORMAT,&encode_surface_trail(&source).unwrap(),&source.map_id).unwrap()).unwrap();
        assert!(state.adopt_source(&mut world,Some((Handle::default(),Mat4::IDENTITY,MapTerrainKind::SparseClipmap))).unwrap());
        let overlay=world.resource::<DynamicClipmapSurfaceOverlay>();
        assert_eq!(overlay.current.as_ref(),Some(&identity(&source)));
        assert_eq!(overlay.trail.as_ref().unwrap().identity,identity(&source));
        assert!(state.entity.is_none(),"Clipmap仍只使用正式高度覆盖通道");
        state.snapshot_started(&mut world).unwrap();
        assert!(world.resource::<DynamicClipmapSurfaceOverlay>().trail.is_none());
    }
    #[test] fn surface_trail_rejects_a_nonrepresentable_projection_before_world_adoption() {
        let mut source=patch();source.source_to_world=Mat4::ZERO.to_cols_array();
        assert!(prepare(SURFACE_TRAIL_FORMAT,&encode_surface_trail(&source).unwrap(),&source.map_id).is_err());
        source.source_to_world=Mat4::IDENTITY.to_cols_array();source.source_to_world[4]=0.5;
        assert!(prepare(SURFACE_TRAIL_FORMAT,&encode_surface_trail(&source).unwrap(),&source.map_id).is_err());
    }
}
