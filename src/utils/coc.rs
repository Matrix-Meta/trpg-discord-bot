use crate::models::types::{CoCRules, RollResult};
use rand::Rng;

/// CoC 7e擲骰
pub fn roll_coc(skill_value: u8, rules: &CoCRules) -> RollResult {
    let roll = rand::thread_rng().gen_range(1..=100);

    let success_level = determine_success_level(roll, skill_value, rules);

    let is_critical_success = roll == rules.critical_success as u16; // Usually 1
    let is_critical_fail = is_critical_failure(roll, skill_value, rules);

    RollResult {
        dice_expr: format!("d100<={}", skill_value),
        rolls: vec![roll],
        modifier: 0,
        total: roll as i32,
        is_critical_success,
        is_critical_fail,
        comparison_result: Some(success_level <= 4),
    }
}

/// 連續擲多次CoC 7e骰
pub fn roll_coc_multi(skill_value: u8, times: u8, rules: &CoCRules) -> Vec<RollResult> {
    let count = times.max(1);
    (0..count).map(|_| roll_coc(skill_value, rules)).collect()
}

/// 根據CoC 7e規則判定成功等級
/// 1: 大成功 (Critical Success)、2: 極限成功 (Extreme Success)、3: 困難成功 (Hard Success)、4: 普通成功 (Regular Success)、5: 失敗 (Failure)、6: 大失敗 (Critical Failure)
pub fn determine_success_level(roll: u16, skill_value: u8, rules: &CoCRules) -> u8 {
    if roll == rules.critical_success as u16 {
        return 1; // 大成功 (Critical success)
    }

    if is_critical_failure(roll, skill_value, rules) {
        return 6; // 大失敗 (Critical failure)
    }

    let hard_success_threshold = skill_value as f32 / rules.skill_divisor_hard as f32;
    let extreme_success_threshold = skill_value as f32 / rules.skill_divisor_extreme as f32;

    if roll == 100 || roll <= extreme_success_threshold as u16 {
        2 // 極限成功 (Extreme success)
    } else if roll <= hard_success_threshold as u16 {
        3 // 困難成功 (Hard success)
    } else if roll <= skill_value as u16 {
        4 // 普通成功 (Regular success)
    } else {
        5 // 失敗 (Failure)
    }
}

/// 大失敗判定標準
pub fn is_critical_failure(roll: u16, skill_value: u8, rules: &CoCRules) -> bool {
    if skill_value < 50 {
        // 技能值低於50%，96-100為大失敗
        roll >= 96
    } else {
        // 技能值50%或以上，100才算大失敗
        roll == rules.critical_fail as u16
    }
}

/// 格式化成功等級為字串
pub fn format_success_level(level: u8) -> String {
    match level {
        1 => "大成功 (Critical Success)".to_string(),
        2 => "極限成功 (Extreme Success)".to_string(),
        3 => "困難成功 (Hard Success)".to_string(),
        4 => "普通成功 (Regular Success)".to_string(),
        5 => "失敗 (Failure)".to_string(),
        6 => "大失敗 (Critical Failure)".to_string(),
        _ => "未知 (Unknown)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_critical_failure_under_50() {
        let rules = CoCRules::default();
        // For skills under 50%, 96-100 should be critical failures
        assert!(is_critical_failure(96, 40, &rules));
        assert!(is_critical_failure(97, 40, &rules));
        assert!(is_critical_failure(98, 40, &rules));
        assert!(is_critical_failure(99, 40, &rules));
        assert!(is_critical_failure(100, 40, &rules));
        // 95 should not be a critical failure
        assert!(!is_critical_failure(95, 40, &rules));
    }

    #[test]
    fn test_is_critical_failure_over_50() {
        let rules = CoCRules::default();
        // For skills 50% or over, only 100 should be a critical failure
        assert!(!is_critical_failure(96, 60, &rules));
        assert!(!is_critical_failure(97, 60, &rules));
        assert!(!is_critical_failure(98, 60, &rules));
        assert!(!is_critical_failure(99, 60, &rules));
        assert!(is_critical_failure(100, 60, &rules));
    }

    #[test]
    fn test_determine_success_level() {
        let rules = CoCRules::default();
        // Critical success
        assert_eq!(determine_success_level(1, 50, &rules), 1);
        // Extreme success (≤ skill/5)
        assert_eq!(determine_success_level(10, 50, &rules), 2);
        // Hard success (≤ skill/2)
        assert_eq!(determine_success_level(25, 50, &rules), 3);
        // Regular success (≤ skill)
        assert_eq!(determine_success_level(50, 50, &rules), 4);
        // Failure
        assert_eq!(determine_success_level(51, 50, &rules), 5);
    }

    #[test]
    fn test_roll_coc_multi() {
        let rules = CoCRules::default();
        let results = roll_coc_multi(60, 5, &rules);
        assert_eq!(results.len(), 5);
        for result in results {
            assert!(result.total >= 1 && result.total <= 100);
        }
    }
}
