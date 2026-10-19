//! The heading-up gyrocompass of the baseline: the dial turns so its top points where the
//! observer looks; it shows the pitch as an attitude line and the sun's bearing. Bearings follow
//! the lighting's convention (0 = north = -Z, 90 = east = +X).

pub const DIAL_DIAMETER: f32 = 112.0;
const READOUT_HEIGHT: f32 = 18.0;
/// Screen space the compass occupies below its anchor, for laying out panels beneath it.
pub const FOOTPRINT_HEIGHT: f32 = DIAL_DIAMETER + READOUT_HEIGHT;
pub const ANCHOR_OFFSET: [f32; 2] = [-10.0, 40.0];

const RIM_INSET: f32 = 2.0;
const TICK_STEP_DEGREES: usize = 15;
const MAJOR_TICK_DEGREES: usize = 45;
const MINOR_TICK_LENGTH: f32 = 5.0;
const MAJOR_TICK_LENGTH: f32 = 9.0;
const LABEL_INSET: f32 = 20.0;
const LABEL_SIZE: f32 = 13.0;
const SUN_INSET: f32 = 34.0;
const SUN_MARKER_RADIUS: f32 = 4.0;
const LUBBER_HALF_WIDTH: f32 = 6.0;
const LUBBER_DEPTH: f32 = 9.0;
/// The attitude line travels this fraction of the dial radius between level and straight up
/// or down.
const HORIZON_TRAVEL: f32 = 0.55;
const HORIZON_HALF_LENGTH: f32 = 0.45;
const PITCH_FULL_SCALE_DEGREES: f32 = 90.0;
const READOUT_SIZE: f32 = 12.0;
const FULL_TURN_DEGREES: u32 = 360;

const DIAL_FILL: egui::Color32 = egui::Color32::from_rgba_premultiplied(14, 17, 23, 210);
const DIAL_RIM: egui::Color32 = egui::Color32::from_rgb(120, 128, 140);
const TICK_COLOR: egui::Color32 = egui::Color32::from_rgb(170, 176, 186);
const NORTH_COLOR: egui::Color32 = egui::Color32::from_rgb(235, 80, 70);
const LABEL_COLOR: egui::Color32 = egui::Color32::from_rgb(220, 224, 230);
const SUN_COLOR: egui::Color32 = egui::Color32::from_rgb(250, 205, 90);
const HORIZON_COLOR: egui::Color32 = egui::Color32::from_rgb(110, 170, 230);
const LUBBER_COLOR: egui::Color32 = egui::Color32::from_rgb(250, 250, 250);

pub fn show(ctx: &egui::Context, yaw: f32, pitch_radians: f32, sun_azimuth_degrees: f32) {
    // Yaw turns the forward vector (0, 0, -1) about +Y, so the compass bearing is -yaw.
    let heading = (-yaw).to_degrees().rem_euclid(360.0);
    let pitch = pitch_radians.to_degrees();
    egui::Area::new(egui::Id::new("navigation_compass"))
        .anchor(egui::Align2::RIGHT_TOP, ANCHOR_OFFSET)
        .interactable(false)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(DIAL_DIAMETER, FOOTPRINT_HEIGHT),
                egui::Sense::hover(),
            );
            let painter = ui.painter_at(rect);
            let center = egui::pos2(rect.center().x, rect.top() + DIAL_DIAMETER / 2.0);
            let radius = DIAL_DIAMETER / 2.0 - RIM_INSET;
            // Screen direction of a world bearing on a heading-up dial (screen y points down).
            let towards = |bearing: f32| {
                let angle = (bearing - heading).to_radians();
                egui::vec2(angle.sin(), -angle.cos())
            };
            painter.circle_filled(center, radius, DIAL_FILL);
            painter.circle_stroke(center, radius, egui::Stroke::new(1.5, DIAL_RIM));
            // Looking up drops the horizon below the dial centre, as on an attitude indicator.
            let horizon_y =
                (pitch / PITCH_FULL_SCALE_DEGREES).clamp(-1.0, 1.0) * radius * HORIZON_TRAVEL;
            let horizon_half = radius * HORIZON_HALF_LENGTH;
            painter.line_segment(
                [
                    center + egui::vec2(-horizon_half, horizon_y),
                    center + egui::vec2(horizon_half, horizon_y),
                ],
                egui::Stroke::new(1.5, HORIZON_COLOR),
            );
            for bearing in (0..360).step_by(TICK_STEP_DEGREES) {
                let length = if bearing % MAJOR_TICK_DEGREES == 0 {
                    MAJOR_TICK_LENGTH
                } else {
                    MINOR_TICK_LENGTH
                };
                let direction = towards(bearing as f32);
                painter.line_segment(
                    [
                        center + direction * (radius - length),
                        center + direction * (radius - RIM_INSET),
                    ],
                    egui::Stroke::new(1.0, TICK_COLOR),
                );
            }
            for (label, bearing, color) in [
                ("N", 0.0, NORTH_COLOR),
                ("E", 90.0, LABEL_COLOR),
                ("S", 180.0, LABEL_COLOR),
                ("W", 270.0, LABEL_COLOR),
            ] {
                painter.text(
                    center + towards(bearing) * (radius - LABEL_INSET),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(LABEL_SIZE),
                    color,
                );
            }
            painter.circle_filled(
                center + towards(sun_azimuth_degrees) * (radius - SUN_INSET),
                SUN_MARKER_RADIUS,
                SUN_COLOR,
            );
            let top = egui::pos2(center.x, center.y - radius);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    top + egui::vec2(-LUBBER_HALF_WIDTH, 0.0),
                    top + egui::vec2(LUBBER_HALF_WIDTH, 0.0),
                    top + egui::vec2(0.0, LUBBER_DEPTH),
                ],
                LUBBER_COLOR,
                egui::Stroke::NONE,
            ));
            // Whole degrees in integers: float formatting would print "-0" and "360".
            let shown_heading = heading.round() as u32 % FULL_TURN_DEGREES;
            let shown_pitch = pitch.round() as i32;
            painter.text(
                egui::pos2(center.x, rect.bottom() - READOUT_HEIGHT / 2.0),
                egui::Align2::CENTER_CENTER,
                format!("{shown_heading:03}°  {shown_pitch:+}°"),
                egui::FontId::monospace(READOUT_SIZE),
                LABEL_COLOR,
            );
        });
}
