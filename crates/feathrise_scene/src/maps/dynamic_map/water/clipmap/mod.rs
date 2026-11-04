//! Geometry Clipmap 水域后端：静水共享正式环带/高度页，样条水复用世界坐标Ribbon。

mod material;
mod runtime;
mod volume;
pub(super) fn install_volume(app:&mut bevy::prelude::App){
    volume::install(app);
    app.add_systems(bevy::prelude::First, runtime::reap_static_water);
}
pub(in crate::maps::dynamic_map) fn add_volume_instance_systems(schedule:&mut bevy::prelude::Schedule) {volume::add_instance_systems(schedule);}

pub use material::ClipmapWaterMaterial;
#[cfg(test)]
pub(crate) use material::test_clipmap_water_material;
pub(crate) use material::{ClipmapRainHeightMaterial, ClipmapWaterMaterialPlugin};
#[cfg(test)]
pub(crate) use runtime::test_clipmap_water_surface;
pub use runtime::{ClipmapSplineWaterSurface, ClipmapWaterSurface};
pub(crate) use runtime::{
    ClipmapWaterRuntime, begin_clipmap_water, clear_clipmap_water, drive_clipmap_water,
    follow_clipmap_water,
};
