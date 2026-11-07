use lamina_model::error::Result;
use lamina_model::*;
use lamina_render::{Snapshot, raster::Options2D};
use lamina_scene::SceneLayer;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ops::place::Target;
use crate::project::Project;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RenderView {
    /// A technical view of the layers, in layer colours (default).
    #[default]
    Layers,
    /// The finished board from the top, with the mask, the finish and the silkscreen.
    Top,
    /// The finished board from the bottom (mirrored).
    Bottom,
    /// A perspective 3D view with the part models.
    #[serde(rename = "3d")]
    View3D,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct Render {
    #[serde(default)]
    pub view: RenderView,
    /// The layers for the `layers` view, for example
    /// `["top", "top.silk", "outline", "ratsnest"]`. Other valid names are `bottom`,
    /// `in1`, `*.mask`, `*.paste`, `*.courtyard`, `*.fab`, `drills`, `keepout` and `drc`.
    #[serde(default)]
    pub layers: Option<Vec<String>>,
    /// Zoom to this box (mm). In 2D views, the image shows only the box. In
    /// the 3D view, the camera fits the box.
    #[serde(default)]
    pub region: Option<Rect>,
    /// 3D view: point the camera at a part (`U1`), a pad (`U1.3`) or a point
    /// (`{"x": 10, "y": 5}`). For a part, the camera fits the part.
    #[serde(default)]
    pub focus: Option<Target>,
    /// 3D view: move the camera closer. 1 is the default fit, 2 is two times
    /// closer, 0.5 is two times further.
    #[serde(default)]
    pub zoom: Option<f32>,
    /// 3D view: do not draw these parts (refdes).
    #[serde(default)]
    pub hide: Option<Vec<String>>,
    /// 3D view: draw only these parts (refdes).
    #[serde(default)]
    pub only: Option<Vec<String>>,
    /// Image width in pixels (default 1400).
    #[serde(default)]
    pub width: Option<u32>,
    /// Image height in pixels (default 1000).
    #[serde(default)]
    pub height: Option<u32>,
    /// 3D camera angle around the board in degrees (default -62).
    #[serde(default)]
    pub yaw: Option<f32>,
    /// 3D camera elevation in degrees (default 38). At 90, the camera looks straight down.
    #[serde(default)]
    pub pitch: Option<f32>,
    /// The file name in cache/render. The default is the name of the view.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Rendered {
    pub path: String,
    pub width: u32,
    pub height: u32,
    #[serde(skip)]
    pub png: Vec<u8>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

pub fn parse_scene_layer(s: &str, copper_layers: u8) -> Option<SceneLayer> {
    match s {
        "outline" => return Some(SceneLayer::Outline),
        "drills" => return Some(SceneLayer::Drills),
        "ratsnest" => return Some(SceneLayer::Ratsnest),
        "drc" => return Some(SceneLayer::Drc),
        "keepout" => return Some(SceneLayer::Keepout),
        _ => {}
    }
    let l: Layer = s.parse().ok()?;
    Some(match l {
        Layer::Copper(c) => {
            if !c.exists_in(copper_layers) {
                return None;
            }
            SceneLayer::Copper(c.index(copper_layers))
        }
        Layer::Mask(sd) => SceneLayer::Mask(sd),
        Layer::Paste(sd) => SceneLayer::Paste(sd),
        Layer::Silk(sd) => SceneLayer::Silk(sd),
        Layer::Courtyard(sd) => SceneLayer::Courtyard(sd),
        Layer::Fab(sd) => SceneLayer::Fab(sd),
        Layer::Outline => SceneLayer::Outline,
    })
}

impl Project {
    pub fn render(&self, p: Render) -> Result<Rendered> {
        let w = p.width.unwrap_or(1400).clamp(64, 4096);
        let h = p.height.unwrap_or(1000).clamp(64, 4096);
        let (_, s2, mut s3) = lamina_scene::build(&self.data, self.root());
        let mut notes = Vec::new();
        if p.view == RenderView::View3D {
            notes.extend(self.filter_instances(&mut s3, p.hide.as_deref(), p.only.as_deref())?);
        }
        let layers = match &p.layers {
            None => None,
            Some(names) => {
                let mut v = Vec::new();
                for n in names {
                    v.push(
                        parse_scene_layer(n, self.data.board.stackup.copper_layers).ok_or_else(
                            || Error::invalid(format!("The layer `{n}` is unknown.")),
                        )?,
                    );
                }
                Some(v)
            }
        };
        let kind = match p.view {
            RenderView::Layers => Snapshot::Layers,
            RenderView::Top => Snapshot::Side(Side::Top),
            RenderView::Bottom => Snapshot::Side(Side::Bottom),
            RenderView::View3D => Snapshot::View3D {
                yaw: p.yaw.unwrap_or(-62.0),
                pitch: p.pitch.unwrap_or(38.0).clamp(-89.9, 90.0),
                frame: self.frame_3d(p.region.as_ref(), p.focus.as_ref())?,
                zoom: p.zoom.unwrap_or(1.0),
            },
        };
        let opts = Options2D {
            width: w,
            height: h,
            layers,
            region: p.region,
        };
        let (png, problems) =
            lamina_render::snapshot(&s2, &s3, kind, w, h, Some(&opts)).map_err(Error::Invalid)?;
        let name = p.name.unwrap_or_else(|| {
            match p.view {
                RenderView::Layers => "layers",
                RenderView::Top => "top",
                RenderView::Bottom => "bottom",
                RenderView::View3D => "3d",
            }
            .to_string()
        });
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path = self.root().join(RENDER_DIR).join(format!("{safe}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| Error::io(&path, e))?;
        std::fs::write(&path, &png).map_err(|e| Error::io(&path, e))?;
        notes.extend(problems);
        if p.view == RenderView::View3D {
            let missing: Vec<&str> = s3
                .instances
                .iter()
                .filter(|i| i.model.is_none() && i.placeholder.is_some())
                .map(|i| i.refdes.as_str())
                .collect();
            if !missing.is_empty() {
                notes.push(format!(
                    "These parts have no 3D model, so they are shown as boxes: {}",
                    missing.join(" ")
                ));
            }
        }
        Ok(Rendered {
            path: path.display().to_string(),
            width: w,
            height: h,
            png,
            notes,
        })
    }
}

impl Project {
    /// The box that the 3D camera fits: the `region`, or the `focus` target.
    /// `None` fits the whole board.
    pub fn frame_3d(
        &self,
        region: Option<&Rect>,
        focus: Option<&Target>,
    ) -> Result<Option<([f32; 3], [f32; 3])>> {
        let t = self.data.board.stackup.thickness as f32;
        let bx = |r: Rect, z0: f32, z1: f32| {
            Some((
                [r.min.x as f32, r.min.y as f32, z0],
                [r.max.x as f32, r.max.y as f32, z1],
            ))
        };
        if let Some(r) = region {
            return Ok(bx(*r, 0.0, t + 3.0));
        }
        let Some(f) = focus else {
            return Ok(None);
        };
        let around = |c: Point| {
            let m = 2.5;
            Rect {
                min: Point::new(c.x - m, c.y - m),
                max: Point::new(c.x + m, c.y + m),
            }
        };
        match f {
            Target::Point(c) => Ok(bx(around(*c), 0.0, t + 2.0)),
            Target::Name(s) if s.contains('.') => {
                let pr: PadRef = s.parse().map_err(Error::Invalid)?;
                let (c, _, _) = self.pad_position(&pr)?;
                Ok(bx(around(c), 0.0, t + 2.0))
            }
            Target::Name(refdes) => {
                let part =
                    self.data.parts.parts.get(refdes).ok_or_else(|| {
                        Error::invalid(format!("The part {refdes} does not exist."))
                    })?;
                let pl = self
                    .data
                    .placement
                    .placements
                    .get(refdes)
                    .ok_or_else(|| Error::invalid(format!("{refdes} is not placed.")))?;
                let fp = self.data.footprints.get(&part.footprint).ok_or_else(|| {
                    Error::invalid(format!("The footprint {} does not exist.", part.footprint))
                })?;
                let local = fp
                    .courtyard_bbox()
                    .unwrap_or_else(|| around(Point::new(0.0, 0.0)));
                let corners = [
                    Point::new(local.min.x, local.min.y),
                    Point::new(local.max.x, local.min.y),
                    Point::new(local.max.x, local.max.y),
                    Point::new(local.min.x, local.max.y),
                ]
                .map(|c| pl.transform(c));
                let mut r = Rect::from_points(corners).unwrap_or_else(|| around(pl.origin()));
                let m = 1.5;
                r.min = Point::new(r.min.x - m, r.min.y - m);
                r.max = Point::new(r.max.x + m, r.max.y + m);
                let h = fp.height.unwrap_or(2.0) as f32;
                Ok(match pl.side {
                    Side::Top => bx(r, 0.0, t + h),
                    Side::Bottom => bx(r, -h, t),
                })
            }
        }
    }

    /// Remove hidden parts from the 3D scene. Returns notes on what is hidden.
    fn filter_instances(
        &self,
        s3: &mut lamina_scene::Scene3D,
        hide: Option<&[String]>,
        only: Option<&[String]>,
    ) -> Result<Vec<String>> {
        let mut notes = Vec::new();
        for r in hide.into_iter().chain(only).flatten() {
            if !self.data.parts.parts.contains_key(r) {
                return Err(Error::invalid(format!("The part {r} does not exist.")));
            }
        }
        if let Some(only) = only {
            s3.instances.retain(|i| only.contains(&i.refdes));
            notes.push(format!("Only these parts are drawn: {}", only.join(" ")));
        }
        if let Some(hide) = hide {
            let before = s3.instances.len();
            s3.instances.retain(|i| !hide.contains(&i.refdes));
            let n = before - s3.instances.len();
            if n > 0 {
                notes.push(format!("{n} part(s) are hidden: {}", hide.join(" ")));
            }
        }
        Ok(notes)
    }
}
