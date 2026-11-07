//! Drawing views of the board as images.

use lamina_model::{Rect, Side};
use lamina_scene::{Scene2D, Scene3D, SceneLayer};

/// The view to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Snapshot {
    /// Technical layer view.
    Layers,
    /// Finished board seen from one side.
    Side(Side),
    /// Perspective 3D view. `frame` is the box that the camera fits, or the
    /// whole scene if it is `None`. `zoom` divides the camera distance: 2 is
    /// two times closer.
    View3D {
        yaw: f32,
        pitch: f32,
        frame: Option<([f32; 3], [f32; 3])>,
        zoom: f32,
    },
}

/// Options of the layer view.
#[derive(Debug, Clone)]
pub struct Options2D {
    pub width: u32,
    pub height: u32,
    /// Layers to draw (default: all).
    pub layers: Option<Vec<SceneLayer>>,
    /// Area of the board to show (default: whole board).
    pub region: Option<Rect>,
}

pub trait Renderer: Send + Sync {
    /// Draw a PNG. The strings are notes about what could not be drawn.
    fn snapshot(
        &self,
        s2: &Scene2D,
        s3: &Scene3D,
        kind: Snapshot,
        width: u32,
        height: u32,
        opts2d: Option<&Options2D>,
    ) -> Result<(Vec<u8>, Vec<String>), String>;
}
