//! Rendering: a CPU rasterizer for 2D snapshots and textures, and wgpu for
//! 3D and the live viewer.

pub mod gpu;
pub mod port;
pub mod raster;
pub mod tess;

pub use gpu::{Camera, Gpu, Gpu2D, Gpu3D, MeshCache, Target};
pub use port::PngRenderer;

use lamina_scene::{Scene2D, Scene3D};

pub use lamina_ports::render::{Options2D, Snapshot};

/// Render a PNG. 2D views work without a GPU. 3D views need a GPU.
pub fn snapshot(
    s2: &Scene2D,
    s3: &Scene3D,
    kind: Snapshot,
    width: u32,
    height: u32,
    opts2d: Option<&raster::Options2D>,
) -> Result<(Vec<u8>, Vec<String>), String> {
    match kind {
        Snapshot::Layers => {
            let def = raster::Options2D {
                width,
                height,
                layers: None,
                region: None,
            };
            let pm = raster::render_layers(s2, opts2d.unwrap_or(&def))
                .ok_or("the image size is not valid")?;
            Ok((raster::png(&pm)?, Vec::new()))
        }
        Snapshot::Side(side) => {
            let pm = raster::render_side(s2, side, width, height)
                .ok_or("the image size is not valid")?;
            Ok((raster::png(&pm)?, Vec::new()))
        }
        Snapshot::View3D {
            yaw,
            pitch,
            frame,
            zoom,
        } => {
            let gpu = Gpu::headless()?;
            let mut cache = MeshCache::default();
            let scene = gpu.upload_3d(s3, s2, &mut cache);
            let aspect = width as f32 / height.max(1) as f32;
            let (lo, hi) = frame.unwrap_or(scene.bounds);
            let mut cam = Camera::frame_aspect(lo, hi, aspect);
            cam.yaw = yaw;
            cam.pitch = pitch;
            cam.distance /= zoom.clamp(0.1, 100.0);
            let t = gpu.target(width, height);
            gpu.draw_3d(&t, &scene, &cam);
            let rgba = gpu.read_rgba(&t)?;
            let mut pm =
                tiny_skia::Pixmap::new(width, height).ok_or("the image size is not valid")?;
            pm.data_mut().copy_from_slice(&rgba);
            let problems = scene
                .problems
                .iter()
                .map(|(r, e)| format!("{r}: the model did not load: {e}"))
                .collect();
            Ok((raster::png(&pm)?, problems))
        }
    }
}
