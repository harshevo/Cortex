//! A camera that orbits a point and eases toward where it is sent.

use eframe::egui::{Pos2, Rect};
use glam::{Mat4, Vec3, Vec4};

use super::instances::Globals;

const FIELD_OF_VIEW: f32 = 0.82;

const NEAREST: f32 = 1.2;
const FARTHEST: f32 = 16.0;

/// The camera never looks straight down the axis it orbits around.
const STEEPEST: f32 = 1.45;

/// Radians of orbit per pixel dragged.
const DRAG_SPEED: f32 = 0.006;

/// Radians per second the galaxy drifts when nobody is steering.
const DRIFT_SPEED: f32 = 0.035;

/// Seconds after the last drag before the drift resumes.
const DRIFT_DELAY: f32 = 2.5;

/// How quickly the camera closes on its goal. Higher is snappier.
const EASE_RATE: f32 = 3.5;

const NEAR_PLANE: f32 = 0.05;
const FAR_PLANE: f32 = 200.0;

/// Moves the world so that `eye` is at the origin looking down -Z at
/// `target`, with +Y up.
fn look_at(eye: Vec3, target: Vec3) -> Mat4 {
    let forward = (target - eye).normalize();
    let right = forward.cross(Vec3::Y).normalize();
    let up = right.cross(forward);

    Mat4::from_cols(
        Vec4::new(right.x, up.x, -forward.x, 0.0),
        Vec4::new(right.y, up.y, -forward.y, 0.0),
        Vec4::new(right.z, up.z, -forward.z, 0.0),
        Vec4::new(-right.dot(eye), -up.dot(eye), forward.dot(eye), 1.0),
    )
}

/// A perspective projection into the GPU's clip space, where depth runs
/// from 0 at the near plane to 1 at the far plane.
fn perspective(field_of_view: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let focal = 1.0 / (field_of_view * 0.5).tan();
    let depth = far / (near - far);

    Mat4::from_cols(
        Vec4::new(focal / aspect, 0.0, 0.0, 0.0),
        Vec4::new(0.0, focal, 0.0, 0.0),
        Vec4::new(0.0, 0.0, depth, -1.0),
        Vec4::new(0.0, 0.0, depth * near, 0.0),
    )
}

/// One moment's view through the camera. Working out the view takes a
/// little arithmetic, so it is done once here and shared by every point
/// that needs placing on screen in the same frame.
pub struct Lens {
    view_projection: Mat4,
    eye: Vec3,
    rect: Rect,
}

impl Lens {
    /// Where `point` appears on screen, or `None` if it is behind the
    /// camera.
    pub fn project(&self, point: Vec3) -> Option<Pos2> {
        let clip = self.view_projection * point.extend(1.0);

        if clip.w <= 0.05 {
            return None;
        }

        let x = clip.x / clip.w * 0.5 + 0.5;
        let y = 0.5 - clip.y / clip.w * 0.5;

        Some(Pos2::new(
            self.rect.left() + x * self.rect.width(),
            self.rect.top() + y * self.rect.height(),
        ))
    }

    /// How many points on screen one world unit covers at `point`.
    pub fn scale_at(&self, point: Vec3) -> f32 {
        let depth = (point - self.eye).length().max(0.05);

        self.rect.height() / (2.0 * (FIELD_OF_VIEW * 0.5).tan() * depth)
    }
}

pub struct Camera {
    yaw: f32,
    pitch: f32,
    distance: f32,
    target: Vec3,

    goal_distance: f32,
    goal_target: Vec3,

    seconds_since_steered: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            yaw: 0.45,
            pitch: 0.16,
            distance: 7.0,
            target: Vec3::ZERO,
            goal_distance: 5.4,
            goal_target: Vec3::ZERO,
            seconds_since_steered: DRIFT_DELAY,
        }
    }
}

impl Camera {
    pub fn orbit(&mut self, pixels_x: f32, pixels_y: f32) {
        self.yaw -= pixels_x * DRAG_SPEED;
        self.pitch = (self.pitch + pixels_y * DRAG_SPEED).clamp(-STEEPEST, STEEPEST);

        self.seconds_since_steered = 0.0;
    }

    /// `factor` below one moves closer, above one moves away.
    pub fn zoom(&mut self, factor: f32) {
        self.goal_distance = (self.goal_distance * factor).clamp(NEAREST, FARTHEST);
    }

    /// Glides to look at `point` from `distance` away.
    pub fn look_at(&mut self, point: Vec3, distance: f32) {
        self.goal_target = point;
        self.goal_distance = distance.clamp(NEAREST, FARTHEST);
    }

    /// Keeps following a point that is itself moving.
    pub fn follow(&mut self, point: Vec3) {
        self.goal_target = point;
    }

    pub fn advance(&mut self, seconds: f32) {
        let ease = 1.0 - (-seconds * EASE_RATE).exp();

        self.target += (self.goal_target - self.target) * ease;
        self.distance += (self.goal_distance - self.distance) * ease;

        self.seconds_since_steered += seconds;

        if self.seconds_since_steered > DRIFT_DELAY {
            self.yaw += seconds * DRIFT_SPEED;
        }
    }

    pub fn position(&self) -> Vec3 {
        let level = self.pitch.cos();

        let direction = Vec3::new(
            level * self.yaw.sin(),
            self.pitch.sin(),
            level * self.yaw.cos(),
        );

        self.target + direction * self.distance
    }

    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        let view = look_at(self.position(), self.target);
        let projection = perspective(FIELD_OF_VIEW, aspect, NEAR_PLANE, FAR_PLANE);

        projection * view
    }

    /// The view into `rect` as it is right now, for finding where points
    /// of the brain appear on screen.
    pub fn lens(&self, rect: Rect) -> Lens {
        Lens {
            view_projection: self.view_projection(rect.aspect_ratio()),
            eye: self.position(),
            rect,
        }
    }

    /// The camera as the shaders need it. `focus` is 1 while a memory is
    /// selected; `activity` is how lit up short-term memory is.
    pub fn globals(
        &self,
        rect: Rect,
        pixels_per_point: f32,
        time: f32,
        focus: f32,
        activity: f32,
    ) -> Globals {
        let aspect = rect.aspect_ratio();

        let position = self.position();
        let forward = (self.target - position).normalize();
        let right = forward.cross(Vec3::Y).normalize();
        let up = right.cross(forward);

        let tan_vertical = (FIELD_OF_VIEW * 0.5).tan();
        let tan_horizontal = tan_vertical * aspect;

        Globals {
            view_projection: self.view_projection(aspect).to_cols_array_2d(),
            camera_right: right.extend(tan_horizontal).to_array(),
            camera_up: up.extend(tan_vertical).to_array(),
            camera_forward: forward.extend(time).to_array(),
            camera_position: position.extend(focus).to_array(),
            viewport: [
                rect.width() * pixels_per_point,
                rect.height() * pixels_per_point,
                0.0,
                0.0,
            ],
            activity: [activity, 0.0, 0.0, 0.0],
        }
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::Vec2;

    use super::*;

    fn rect() -> Rect {
        Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(800.0, 600.0))
    }

    #[test]
    fn the_target_is_at_the_centre_of_the_view() {
        let camera = Camera::default();

        let centre = camera.lens(rect()).project(Vec3::ZERO).unwrap();

        assert!((centre.x - 500.0).abs() < 0.01);
        assert!((centre.y - 350.0).abs() < 0.01);
    }

    #[test]
    fn a_point_behind_the_camera_is_not_on_screen() {
        let camera = Camera::default();

        let behind = camera.position() * 2.0;

        assert!(camera.lens(rect()).project(behind).is_none());
    }

    #[test]
    fn the_camera_eases_to_where_it_is_sent() {
        let mut camera = Camera::default();

        camera.look_at(Vec3::new(1.0, 0.0, 0.0), 3.0);

        for _ in 0..600 {
            camera.advance(1.0 / 60.0);
        }

        assert!((camera.target - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-3);
        assert!((camera.distance - 3.0).abs() < 1e-3);
    }
}
