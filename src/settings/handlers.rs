use crossterm::event::KeyCode;
use crate::common::SettingsState;
use super::clamp_poll_interval;

/// Clamp CPU threshold to valid range.
pub fn clamp_cpu_threshold(value: f32) -> f32 {
    value.clamp(10.0, 100.0)
}

/// Clamp duration to valid range.
pub fn clamp_duration_secs(value: u64) -> u64 {
    value.clamp(1, 30)
}

/// Clamp per-core threshold to valid range.
pub fn clamp_per_core_threshold(value: f32) -> f32 {
    value.clamp(30.0, 100.0)
}

/// Clamp relative multiplier to valid range.
pub fn clamp_relative_multiplier(value: f32) -> f32 {
    value.clamp(2.0, 50.0)
}

/// Clamp EMA alpha to valid range.
pub fn clamp_ema_alpha(value: f32) -> f32 {
    value.clamp(0.05, 1.0)
}

/// Clamp grace period to valid range.
pub fn clamp_grace_period_secs(value: u64) -> u64 {
    value.clamp(0, 60)
}

/// Clamp adaptive sensitivity to valid range.
pub fn clamp_adaptive_sensitivity(value: f32) -> f32 {
    value.clamp(0.0, 2.0)
}

/// Handle settings menu navigation and adjustment
pub fn handle_key(state: &mut SettingsState, key: KeyCode) {
    match key {
        KeyCode::Up => {
            if state.selected_option > 0 {
                state.selected_option -= 1;
            }
        }
        KeyCode::Down => {
            if state.selected_option < SettingsState::MAX_OPTION {
                state.selected_option += 1;
            }
        }
        KeyCode::Left => {
            match state.selected_option {
                0 => {
                    state.poll_interval_ms = clamp_poll_interval(state.poll_interval_ms.saturating_sub(50));
                }
                1 => {
                    state.hide_self = !state.hide_self;
                }
                2 => {
                    state.priority_guard_enabled = !state.priority_guard_enabled;
                }
                3 => {
                    state.priority_guard_cpu_threshold = clamp_cpu_threshold(state.priority_guard_cpu_threshold - 5.0);
                }
                4 => {
                    state.priority_guard_duration_secs = clamp_duration_secs(state.priority_guard_duration_secs.saturating_sub(1));
                }
                5 => {
                    state.priority_guard_per_core_threshold = clamp_per_core_threshold(state.priority_guard_per_core_threshold - 5.0);
                }
                6 => {
                    state.priority_guard_relative_multiplier = clamp_relative_multiplier(state.priority_guard_relative_multiplier - 1.0);
                }
                7 => {
                    state.priority_guard_ema_alpha = clamp_ema_alpha(state.priority_guard_ema_alpha - 0.05);
                }
                8 => {
                    state.priority_guard_grace_period_secs = clamp_grace_period_secs(state.priority_guard_grace_period_secs.saturating_sub(1));
                }
                9 => {
                    state.priority_guard_adaptive_sensitivity = clamp_adaptive_sensitivity(state.priority_guard_adaptive_sensitivity - 0.1);
                }
                10 => {
                    // Exemptions: no Left/Right adjustment (Enter to edit in ExemptionEditor mode)
                }
                _ => {}
            }
        }
        KeyCode::Right => {
            match state.selected_option {
                0 => {
                    state.poll_interval_ms = clamp_poll_interval(state.poll_interval_ms + 50);
                }
                1 => {
                    state.hide_self = !state.hide_self;
                }
                2 => {
                    state.priority_guard_enabled = !state.priority_guard_enabled;
                }
                3 => {
                    state.priority_guard_cpu_threshold = clamp_cpu_threshold(state.priority_guard_cpu_threshold + 5.0);
                }
                4 => {
                    state.priority_guard_duration_secs = clamp_duration_secs(state.priority_guard_duration_secs + 1);
                }
                5 => {
                    state.priority_guard_per_core_threshold = clamp_per_core_threshold(state.priority_guard_per_core_threshold + 5.0);
                }
                6 => {
                    state.priority_guard_relative_multiplier = clamp_relative_multiplier(state.priority_guard_relative_multiplier + 1.0);
                }
                7 => {
                    state.priority_guard_ema_alpha = clamp_ema_alpha(state.priority_guard_ema_alpha + 0.05);
                }
                8 => {
                    state.priority_guard_grace_period_secs = clamp_grace_period_secs(state.priority_guard_grace_period_secs + 1);
                }
                9 => {
                    state.priority_guard_adaptive_sensitivity = clamp_adaptive_sensitivity(state.priority_guard_adaptive_sensitivity + 0.1);
                }
                10 => {
                    // Exemptions: no Left/Right adjustment (Enter to edit in ExemptionEditor mode)
                }
                _ => {}
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_state() -> SettingsState {
        SettingsState::new()
    }

    #[test]
    fn down_moves_to_next_option() {
        let mut state = default_state();
        assert_eq!(state.selected_option, 0);
        handle_key(&mut state, KeyCode::Down);
        assert_eq!(state.selected_option, 1);
    }

    #[test]
    fn down_at_last_stays() {
        let mut state = default_state();
        state.selected_option = SettingsState::MAX_OPTION;
        handle_key(&mut state, KeyCode::Down);
        assert_eq!(state.selected_option, SettingsState::MAX_OPTION);
    }

    #[test]
    fn up_moves_to_previous_option() {
        let mut state = default_state();
        state.selected_option = 1;
        handle_key(&mut state, KeyCode::Up);
        assert_eq!(state.selected_option, 0);
    }

    #[test]
    fn up_at_first_stays() {
        let mut state = default_state();
        handle_key(&mut state, KeyCode::Up);
        assert_eq!(state.selected_option, 0);
    }

    #[test]
    fn right_increases_poll_interval() {
        let mut state = default_state();
        state.selected_option = 0;
        let before = state.poll_interval_ms;
        handle_key(&mut state, KeyCode::Right);
        assert_eq!(state.poll_interval_ms, before + 50);
    }

    #[test]
    fn left_decreases_poll_interval() {
        let mut state = default_state();
        state.selected_option = 0;
        let before = state.poll_interval_ms;
        handle_key(&mut state, KeyCode::Left);
        assert_eq!(state.poll_interval_ms, before - 50);
    }

    #[test]
    fn poll_interval_clamped_at_min() {
        let mut state = default_state();
        state.selected_option = 0;
        state.poll_interval_ms = 50;
        handle_key(&mut state, KeyCode::Left);
        assert_eq!(state.poll_interval_ms, 50);
    }

    #[test]
    fn poll_interval_clamped_at_max() {
        let mut state = default_state();
        state.selected_option = 0;
        state.poll_interval_ms = 10000;
        handle_key(&mut state, KeyCode::Right);
        assert_eq!(state.poll_interval_ms, 10000);
    }

    #[test]
    fn right_toggles_hide_self() {
        let mut state = default_state();
        state.selected_option = 1;
        assert!(!state.hide_self);
        handle_key(&mut state, KeyCode::Right);
        assert!(state.hide_self);
    }

    #[test]
    fn left_toggles_hide_self() {
        let mut state = default_state();
        state.selected_option = 1;
        state.hide_self = true;
        handle_key(&mut state, KeyCode::Left);
        assert!(!state.hide_self);
    }

    #[test]
    fn unknown_key_is_noop() {
        let mut state = default_state();
        let interval = state.poll_interval_ms;
        handle_key(&mut state, KeyCode::Char('z'));
        assert_eq!(state.poll_interval_ms, interval);
        assert_eq!(state.selected_option, 0);
    }
}

#[cfg(test)]
mod fuzz_tests {
    use super::*;
    use proptest::prelude::*;

    fn arb_keycode() -> impl Strategy<Value = KeyCode> {
        prop_oneof![
            Just(KeyCode::Up),
            Just(KeyCode::Down),
            Just(KeyCode::Left),
            Just(KeyCode::Right),
            Just(KeyCode::Tab),
            Just(KeyCode::Esc),
            Just(KeyCode::Backspace),
            any::<char>().prop_map(KeyCode::Char),
        ]
    }

    proptest! {
        #[test]
        fn fuzz_settings_keys_invariants(keys in prop::collection::vec(arb_keycode(), 1..100)) {
            let mut state = SettingsState::new();
            for key in keys {
                handle_key(&mut state, key);
                prop_assert!(state.selected_option <= SettingsState::MAX_OPTION, "selected_option out of range: {}", state.selected_option);
                prop_assert!(state.poll_interval_ms >= 50, "poll_interval below min: {}", state.poll_interval_ms);
                prop_assert!(state.poll_interval_ms <= 10000, "poll_interval above max: {}", state.poll_interval_ms);
            }
        }
    }
}
