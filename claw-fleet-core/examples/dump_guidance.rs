fn main() {
    println!(
        "=== zh / 老板 ===\n{}\n",
        claw_fleet_core::interaction_mode::render_guidance("老板", "zh")
    );
    println!(
        "=== en / (empty) ===\n{}",
        claw_fleet_core::interaction_mode::render_guidance("", "en")
    );
}
