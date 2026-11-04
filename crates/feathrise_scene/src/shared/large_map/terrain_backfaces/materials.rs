//! 保留地形材质扩展与实例坐标，只临时改变标准材质的背面显示。

use crate::maps::dynamic_map::{ClipmapHeightMaterial, SparseClipmapSurfaceMaterial};
use bevy::prelude::*;
use feathrise_environment::terrain_surface::TerrainSurfaceMaterial;

pub(super) trait BackfaceMaterial: Material + Clone {
    fn standard_mut(&mut self) -> &mut StandardMaterial;
}

impl BackfaceMaterial for StandardMaterial {
    fn standard_mut(&mut self) -> &mut StandardMaterial {
        self
    }
}

impl BackfaceMaterial for TerrainSurfaceMaterial {
    fn standard_mut(&mut self) -> &mut StandardMaterial {
        &mut self.base
    }
}

impl BackfaceMaterial for ClipmapHeightMaterial {
    fn standard_mut(&mut self) -> &mut StandardMaterial {
        &mut self.base
    }
}

impl BackfaceMaterial for SparseClipmapSurfaceMaterial {
    fn standard_mut(&mut self) -> &mut StandardMaterial {
        &mut self.base.base
    }
}
