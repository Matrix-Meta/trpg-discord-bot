use crate::models::types::{DiceRoll, DnDRules, RollResult};
use rand::Rng;
use regex::Regex;

/// 表達式解析（例如 "2d6+1 >= 10"）
pub fn parse_dice_expr(expr: &str, rules: &DnDRules) -> Result<DiceRoll, String> {
    let expr = expr.trim();
    let re = Regex::new(r"^(\d*)d(\d+)([+\-]\d+)?(?:\s*(>=|<=|>|<)\s*(\d+))?$")
        .map_err(|_| "無效的正規表達式")?;

    let captures = re
        .captures(expr)
        .ok_or_else(|| "無效的擲骰表達式格式".to_string())?;

    let count_str = captures.get(1).map_or("1", |m| m.as_str());
    let count = if count_str.is_empty() {
        1
    } else {
        count_str
            .parse::<u8>()
            .map_err(|_| "無效擲骰數".to_string())?
    };

    if count == 0 {
        return Err("擲骰數必須至少為 1".to_string());
    }

    if count > rules.max_dice_count {
        return Err(format!("擲骰數過多（最大 {}）", rules.max_dice_count));
    }

    let sides = captures
        .get(2)
        .ok_or_else(|| "缺少骰子面數".to_string())?
        .as_str()
        .parse::<u16>()
        .map_err(|_| "無效擲骰面數".to_string())?;

    if sides < 2 {
        return Err("擲骰面數必須至少為 2".to_string());
    }

    if sides > rules.max_dice_sides {
        return Err(format!("擲骰面數過多（最大 {}）", rules.max_dice_sides));
    }

    let modifier = captures
        .get(3)
        .map(|m| m.as_str())
        .unwrap_or("0")
        .parse::<i32>()
        .map_err(|_| "Invalid modifier".to_string())?;

    let comparison = if let Some(op_match) = captures.get(4) {
        let op = op_match.as_str().to_string();
        let value = captures
            .get(5)
            .ok_or_else(|| "Missing comparison value".to_string())?
            .as_str()
            .parse::<i32>()
            .map_err(|_| "Invalid comparison value".to_string())?;
        Some((op, value))
    } else {
        None
    };

    Ok(DiceRoll {
        count,
        sides,
        modifier,
        comparison,
    })
}

/// 指定邊數擲單骰
pub fn roll_single_dice(sides: u16) -> u16 {
    rand::thread_rng().gen_range(1..=sides)
}

/// 擲多顆骰子並返回結果
pub fn roll_dice(dice: &DiceRoll) -> RollResult {
    let mut rolls = Vec::new();

    for _ in 0..dice.count {
        rolls.push(roll_single_dice(dice.sides));
    }

    let total = rolls.iter().map(|&r| r as i32).sum::<i32>() + dice.modifier;

    // 判定是否為大成功或失敗
    let is_critical_success = dice.sides == 20 && rolls.contains(&20);
    let is_critical_fail = dice.sides == 20 && rolls.contains(&1);

    // 評估比較條件（如果存在）
    let comparison_result = match &dice.comparison {
        Some((op, value)) => match op.as_str() {
            ">=" => Some(total >= *value),
            ">" => Some(total > *value),
            "<=" => Some(total <= *value),
            "<" => Some(total < *value),
            _ => None,
        },
        None => None,
    };

    RollResult {
        dice_expr: format_dice_expr(dice),
        rolls,
        modifier: dice.modifier,
        total,
        is_critical_success,
        is_critical_fail,
        comparison_result,
    }
}

fn format_dice_expr(dice: &DiceRoll) -> String {
    let modifier = if dice.modifier == 0 {
        String::new()
    } else if dice.modifier > 0 {
        format!("+{}", dice.modifier)
    } else {
        dice.modifier.to_string()
    };

    format!("{}d{}{}", dice.count, dice.sides, modifier)
}

/// 解析並擲多顆骰子表達式（用於連續擲骰）
pub fn roll_multiple_dice(
    expr: &str,
    max_rolls: u8,
    rules: &DnDRules,
) -> Result<Vec<RollResult>, String> {
    let re = Regex::new(r"^(?:\+)?(\d+)\s+(.+)$").map_err(|_| "無效的正則表達式")?;

    if let Some(captures) = re.captures(expr.trim()) {
        let count = captures
            .get(1)
            .ok_or_else(|| "缺少擲骰數量".to_string())?
            .as_str()
            .parse::<u8>()
            .map_err(|_| "無效的擲骰數量".to_string())?;

        if count == 0 {
            return Err("擲骰數量必須至少為 1".to_string());
        }

        if count > max_rolls {
            return Err(format!("擲骰數量過多（最大 {}）", max_rolls));
        }

        let expr = captures
            .get(2)
            .ok_or_else(|| "缺少擲骰表達式".to_string())?
            .as_str()
            .trim();

        let parsed_dice = parse_dice_expr(expr, rules)?;

        let mut results = Vec::new();
        for _ in 0..count {
            results.push(roll_dice(&parsed_dice));
        }

        Ok(results)
    } else {
        let parsed_dice = parse_dice_expr(expr, rules)?;
        Ok(vec![roll_dice(&parsed_dice)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dice_expr() {
        let rules = DnDRules::default();
        let dice = parse_dice_expr("2d6+1", &rules).unwrap();
        assert_eq!(dice.count, 2);
        assert_eq!(dice.sides, 6);
        assert_eq!(dice.modifier, 1);
    }

    #[test]
    fn test_parse_dice_expr_without_modifier() {
        let rules = DnDRules::default();
        let dice = parse_dice_expr("d20", &rules).unwrap();
        assert_eq!(dice.count, 1);
        assert_eq!(dice.sides, 20);
        assert_eq!(dice.modifier, 0);
    }

    #[test]
    fn test_roll_dice() {
        let dice = DiceRoll {
            count: 1,
            sides: 6,
            modifier: 0,
            comparison: None,
        };

        let result = roll_dice(&dice);
        assert!(result.rolls[0] >= 1 && result.rolls[0] <= 6);
        assert_eq!(result.total, result.rolls[0] as i32);
    }

    #[test]
    fn test_roll_multiple_dice() {
        let rules = DnDRules::default();
        let results = roll_multiple_dice("+3 d4", rules.max_dice_count, &rules).unwrap();
        assert_eq!(results.len(), 3);
    }
}
