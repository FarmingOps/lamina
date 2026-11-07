//! The [`Renderer`] port, implemented with [`snapshot`](crate::snapshot).

use lamina_ports::render::{Options2D, Renderer, Snapshot};
use lamina_scene::{Scene2D, Scene3D};

/// Draws PNG views of the board. 2D views use the CPU rasterizer. 3D views
/// use the GPU.
#[derive(Debug, Clone, Copy, Default)]
pub struct PngRenderer;

impl Renderer for PngRenderer {
    fn snapshot(
        &self,
        s2: &Scene2D,
        s3: &Scene3D,
        kind: Snapshot,
        width: u32,
        height: u32,
        opts2d: Option<&Options2D>,
    ) -> Result<(Vec<u8>, Vec<String>), String> {
        crate::snapshot(s2, s3, kind, width, height, opts2d)
    }
}
