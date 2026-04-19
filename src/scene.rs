//! Root scene type.

use oxideav_core::TimeBase;

use crate::audio::AudioCue;
use crate::duration::SceneDuration;
use crate::object::{Canvas, SceneObject};

/// Top-level scene — a canvas + a timeline of objects and audio cues.
#[derive(Clone, Debug)]
pub struct Scene {
    pub canvas: Canvas,
    pub duration: SceneDuration,
    /// Rational time base — all timestamps in the scene are integer
    /// multiples of this. Matches `oxideav-core`'s `TimeBase`.
    pub time_base: TimeBase,
    /// Audio mix-bus sample rate.
    pub sample_rate: u32,
    pub background: Background,
    /// Z-ordered object list. Objects are composited in `z_order`
    /// ascending order; ties break by list position.
    pub objects: Vec<SceneObject>,
    pub audio: Vec<AudioCue>,
    pub metadata: Metadata,
}

impl Default for Scene {
    fn default() -> Self {
        Scene {
            canvas: Canvas::raster(1920, 1080),
            duration: SceneDuration::Finite(0),
            time_base: TimeBase::new(1, 1_000),
            sample_rate: 48_000,
            background: Background::default(),
            objects: Vec::new(),
            audio: Vec::new(),
            metadata: Metadata::default(),
        }
    }
}

impl Scene {
    /// Sort the object list by z-order, preserving insertion order
    /// within ties. Idempotent — call before rendering if the scene
    /// was built incrementally.
    pub fn sort_by_z_order(&mut self) {
        self.objects.sort_by_key(|o| o.z_order);
    }

    /// Return objects live at time `t`, in paint order (z-order
    /// ascending).
    pub fn visible_at(&self, t: crate::duration::TimeStamp) -> Vec<&SceneObject> {
        let mut refs: Vec<&SceneObject> = self
            .objects
            .iter()
            .filter(|o| o.lifetime.is_live_at(t))
            .collect();
        refs.sort_by_key(|o| o.z_order);
        refs
    }
}

/// What fills the canvas below the z-ordered object list.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum Background {
    /// Fully transparent. Export paths emit RGBA output.
    Transparent,
    /// Solid colour, `0xRRGGBBAA`.
    Solid(u32),
    /// Vertical or horizontal gradient between two colours.
    LinearGradient {
        from: u32,
        to: u32,
        /// Direction in degrees clockwise from 12 o'clock (0° = top,
        /// 90° = right, …).
        angle_deg: f32,
    },
    /// Bitmap background — cover or contain fit is up to the
    /// renderer's layout policy.
    Image(String),
}

impl Default for Background {
    fn default() -> Self {
        Background::Solid(0x000000FF)
    }
}

/// Scene-level metadata. Carried through to exports when the target
/// format supports it (PDF document info dict, MP4 `meta` box, etc).
#[derive(Clone, Debug, Default)]
pub struct Metadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Vec<String>,
    /// Producing tool name.
    pub producer: Option<String>,
    /// ISO-8601 string; not parsed here so exporters can pass it
    /// through unchanged.
    pub created_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{animation::Animation, duration::Lifetime, id::ObjectId, object::SceneObject};

    #[test]
    fn visible_at_respects_lifetime() {
        let mut scene = Scene {
            duration: SceneDuration::Finite(1000),
            ..Scene::default()
        };
        scene.objects.push(SceneObject {
            id: ObjectId::new(1),
            lifetime: Lifetime {
                start: 100,
                end: Some(200),
            },
            ..SceneObject::default()
        });
        scene.objects.push(SceneObject {
            id: ObjectId::new(2),
            lifetime: Lifetime::default(),
            ..SceneObject::default()
        });
        let v = scene.visible_at(50);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, ObjectId::new(2));

        let v = scene.visible_at(150);
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn sort_by_z_order_stable_ties() {
        let mut scene = Scene::default();
        scene.objects.push(SceneObject {
            id: ObjectId::new(1),
            z_order: 5,
            animations: vec![Animation::new(
                crate::animation::AnimatedProperty::Opacity,
                Vec::new(),
                crate::animation::Easing::Linear,
                crate::animation::Repeat::Once,
            )],
            ..SceneObject::default()
        });
        scene.objects.push(SceneObject {
            id: ObjectId::new(2),
            z_order: 5,
            ..SceneObject::default()
        });
        scene.objects.push(SceneObject {
            id: ObjectId::new(3),
            z_order: 1,
            ..SceneObject::default()
        });
        scene.sort_by_z_order();
        assert_eq!(scene.objects[0].id, ObjectId::new(3));
        assert_eq!(scene.objects[1].id, ObjectId::new(1));
        assert_eq!(scene.objects[2].id, ObjectId::new(2));
    }
}
