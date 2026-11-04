//! 角色、传送点和摆放物共用持久承托关联；地图实例坐标在运行时另行叠加。
mod resolve;
#[cfg(test)]
mod tests;
mod types;
pub use resolve::TerrainBindingIndex;
pub use types::MapTerrainBinding;
