//! Shared fixed-point expression trees for conditions and modification rules.

use crate::modification::{ModificationId, ModificationSourceRef, ModificationTag};

pub type ModificationTarget = String;
pub type MetricId = String;
pub type HostFunctionId = String;
pub type HostPredicateId = String;
pub type TextKey = String;

const FIXED_SCALE: i64 = 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub struct Fixed(i64);

impl Fixed {
    pub const ZERO: Self = Self(0);

    pub const fn units(value: i64) -> Self {
        Self(value * FIXED_SCALE)
    }

    pub const fn milli(value: i64) -> Self {
        Self(value)
    }

    pub const fn new(units: i64, millis: i64) -> Self {
        Self(units * FIXED_SCALE + millis)
    }

    pub const fn raw_millis(self) -> i64 {
        self.0
    }

    pub fn checked_div(self, rhs: Self) -> Option<Self> {
        if rhs.0 == 0 {
            return None;
        }
        Some(Self::from_i128_saturating(
            (self.0 as i128 * FIXED_SCALE as i128) / rhs.0 as i128,
        ))
    }

    pub fn fixed_mul(self, rhs: Self) -> Self {
        Self::from_i128_saturating((self.0 as i128 * rhs.0 as i128) / FIXED_SCALE as i128)
    }

    fn from_i128_saturating(value: i128) -> Self {
        Self(value.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
    }
}

impl std::ops::Add for Fixed {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl std::ops::Sub for Fixed {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0.saturating_sub(rhs.0))
    }
}

impl std::iter::Sum for Fixed {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, |acc, value| acc + value)
    }
}

/// Host-provided values and functions used while evaluating expression trees.
pub trait ModificationEvalContext {
    fn metric_value(&self, id: &str) -> Option<Fixed>;
    fn host_function_value(&self, id: &str, args: &[Fixed]) -> Option<Fixed>;
    fn host_predicate_value(&self, id: &str, args: &[Fixed]) -> Option<bool>;
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModificationEvalError {
    UnknownMetric(MetricId),
    UnknownHostFunction(HostFunctionId),
    UnknownHostPredicate(HostPredicateId),
    DivisionByZero,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModifierProjection {
    pub target: ModificationTarget,
    pub base_add: Option<ValueExpr>,
    pub multiplier: Option<ValueExpr>,
    pub add: Option<ValueExpr>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ValueExpr {
    Const(Fixed),
    Metric(MetricId),
    Add(Vec<ValueExpr>),
    Sub(Box<ValueExpr>, Box<ValueExpr>),
    Mul(Box<ValueExpr>, Box<ValueExpr>),
    Div(Box<ValueExpr>, Box<ValueExpr>),
    Max(Box<ValueExpr>, Box<ValueExpr>),
    Min(Box<ValueExpr>, Box<ValueExpr>),
    Clamp {
        value: Box<ValueExpr>,
        min: Box<ValueExpr>,
        max: Box<ValueExpr>,
    },
    /// The expression is representable, but UI should use the provided label
    /// unless a caller explicitly asks for full debug/detail display.
    Summarized {
        label: TextKey,
        mode: ExprDisplayMode,
        inner: Box<ValueExpr>,
    },
    /// The expression is host-defined and must be evaluated through a registry.
    HostFunction {
        id: HostFunctionId,
        args: Vec<ValueExpr>,
        display: HostExprDisplay,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum BoolExpr {
    True,
    False,
    All(Vec<BoolExpr>),
    Any(Vec<BoolExpr>),
    OneOf(Vec<BoolExpr>),
    Not(Box<BoolExpr>),
    Compare {
        left: ValueExpr,
        op: CompareOp,
        right: ValueExpr,
    },
    Summarized {
        label: TextKey,
        mode: ExprDisplayMode,
        inner: Box<BoolExpr>,
    },
    HostPredicate {
        id: HostPredicateId,
        args: Vec<ValueExpr>,
        display: HostExprDisplay,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub enum ExprDisplayMode {
    #[default]
    SummaryOnly,
    Full,
    Hidden,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub struct HostExprDisplay {
    pub label: TextKey,
    pub mode: ExprDisplayMode,
}

impl ValueExpr {
    pub fn evaluate(
        &self,
        ctx: &dyn ModificationEvalContext,
    ) -> Result<Fixed, ModificationEvalError> {
        match self {
            ValueExpr::Const(value) => Ok(*value),
            ValueExpr::Metric(id) => ctx
                .metric_value(id)
                .ok_or_else(|| ModificationEvalError::UnknownMetric(id.clone())),
            ValueExpr::Add(children) => children
                .iter()
                .try_fold(Fixed::ZERO, |acc, child| Ok(acc + child.evaluate(ctx)?)),
            ValueExpr::Sub(left, right) => Ok(left.evaluate(ctx)? - right.evaluate(ctx)?),
            ValueExpr::Mul(left, right) => Ok(left.evaluate(ctx)?.fixed_mul(right.evaluate(ctx)?)),
            ValueExpr::Div(left, right) => {
                let denominator = right.evaluate(ctx)?;
                left.evaluate(ctx)?
                    .checked_div(denominator)
                    .ok_or(ModificationEvalError::DivisionByZero)
            }
            ValueExpr::Max(left, right) => Ok(left.evaluate(ctx)?.max(right.evaluate(ctx)?)),
            ValueExpr::Min(left, right) => Ok(left.evaluate(ctx)?.min(right.evaluate(ctx)?)),
            ValueExpr::Clamp { value, min, max } => Ok(value
                .evaluate(ctx)?
                .clamp(min.evaluate(ctx)?, max.evaluate(ctx)?)),
            ValueExpr::Summarized { inner, .. } => inner.evaluate(ctx),
            ValueExpr::HostFunction { id, args, .. } => {
                let args = evaluate_args(args, ctx)?;
                ctx.host_function_value(id, &args)
                    .ok_or_else(|| ModificationEvalError::UnknownHostFunction(id.clone()))
            }
        }
    }
}

impl BoolExpr {
    pub fn evaluate(
        &self,
        ctx: &dyn ModificationEvalContext,
    ) -> Result<bool, ModificationEvalError> {
        match self {
            BoolExpr::True => Ok(true),
            BoolExpr::False => Ok(false),
            BoolExpr::All(children) => {
                for child in children {
                    if !child.evaluate(ctx)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            BoolExpr::Any(children) => {
                for child in children {
                    if child.evaluate(ctx)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            BoolExpr::OneOf(children) => {
                let mut count = 0;
                for child in children {
                    if child.evaluate(ctx)? {
                        count += 1;
                    }
                }
                Ok(count == 1)
            }
            BoolExpr::Not(child) => Ok(!child.evaluate(ctx)?),
            BoolExpr::Compare { left, op, right } => {
                let left = left.evaluate(ctx)?;
                let right = right.evaluate(ctx)?;
                Ok(match op {
                    CompareOp::Eq => left == right,
                    CompareOp::Ne => left != right,
                    CompareOp::Lt => left < right,
                    CompareOp::Lte => left <= right,
                    CompareOp::Gt => left > right,
                    CompareOp::Gte => left >= right,
                })
            }
            BoolExpr::Summarized { inner, .. } => inner.evaluate(ctx),
            BoolExpr::HostPredicate { id, args, .. } => {
                let args = evaluate_args(args, ctx)?;
                ctx.host_predicate_value(id, &args)
                    .ok_or_else(|| ModificationEvalError::UnknownHostPredicate(id.clone()))
            }
        }
    }
}

impl ModifierProjection {
    pub fn evaluate(
        &self,
        ctx: &dyn ModificationEvalContext,
    ) -> Result<EvaluatedModifierProjection, ModificationEvalError> {
        Ok(EvaluatedModifierProjection {
            target: self.target.clone(),
            base_add: evaluate_optional_value(self.base_add.as_ref(), ctx)?,
            multiplier: evaluate_optional_value(self.multiplier.as_ref(), ctx)?,
            add: evaluate_optional_value(self.add.as_ref(), ctx)?,
        })
    }
}

impl crate::modification::ModificationRule {
    pub fn evaluate(
        &self,
        ctx: &dyn ModificationEvalContext,
    ) -> Result<Option<EvaluatedModification>, ModificationEvalError> {
        if let Some(when) = &self.when
            && !when.evaluate(ctx)?
        {
            return Ok(None);
        }

        let projections = self
            .projections
            .iter()
            .map(|projection| projection.evaluate(ctx))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Some(EvaluatedModification {
            id: self.id.clone(),
            label: self.label.clone(),
            source: self.source.clone(),
            tags: self.tags.clone(),
            projections,
        }))
    }
}

fn evaluate_optional_value(
    expr: Option<&ValueExpr>,
    ctx: &dyn ModificationEvalContext,
) -> Result<Fixed, ModificationEvalError> {
    expr.map(|expr| expr.evaluate(ctx))
        .unwrap_or(Ok(Fixed::ZERO))
}

fn evaluate_args(
    args: &[ValueExpr],
    ctx: &dyn ModificationEvalContext,
) -> Result<Vec<Fixed>, ModificationEvalError> {
    args.iter().map(|arg| arg.evaluate(ctx)).collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EvaluatedModifierProjection {
    pub target: ModificationTarget,
    pub base_add: Fixed,
    pub multiplier: Fixed,
    pub add: Fixed,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EvaluatedModification {
    pub id: ModificationId,
    pub label: String,
    pub source: ModificationSourceRef,
    pub tags: Vec<ModificationTag>,
    pub projections: Vec<EvaluatedModifierProjection>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modification::{ModificationRule, ModificationSourceRef};
    use std::collections::HashMap;

    #[derive(Default)]
    struct TestEvalContext {
        metrics: HashMap<String, Fixed>,
    }

    impl TestEvalContext {
        fn with_metrics(items: &[(&str, Fixed)]) -> Self {
            Self {
                metrics: items
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), *value))
                    .collect(),
            }
        }
    }

    impl ModificationEvalContext for TestEvalContext {
        fn metric_value(&self, id: &str) -> Option<Fixed> {
            self.metrics.get(id).copied()
        }

        fn host_function_value(&self, id: &str, args: &[Fixed]) -> Option<Fixed> {
            match id {
                "sum" => Some(args.iter().copied().sum()),
                "weighted_excess" => Some((args[0] - args[1]).max(Fixed::ZERO).fixed_mul(args[2])),
                _ => None,
            }
        }

        fn host_predicate_value(&self, id: &str, args: &[Fixed]) -> Option<bool> {
            match id {
                "positive" => Some(args.iter().all(|value| *value > Fixed::ZERO)),
                "between" => Some(args[0] >= args[1] && args[0] <= args[2]),
                _ => None,
            }
        }
    }

    fn boxed_value(expr: ValueExpr) -> Box<ValueExpr> {
        Box::new(expr)
    }

    fn boxed_bool(expr: BoolExpr) -> Box<BoolExpr> {
        Box::new(expr)
    }

    fn assert_fixed_eq(actual: Fixed, expected: Fixed) {
        assert_eq!(
            actual.raw_millis(),
            expected.raw_millis(),
            "expected {expected:?}, got {actual:?}"
        );
    }

    fn source_ref() -> ModificationSourceRef {
        ModificationSourceRef {
            id: "technology.urban_planning".into(),
            label: "Urban Planning".into(),
            kind: Some("technology".into()),
        }
    }

    #[test]
    fn fixed_uses_milli_precision_and_deterministic_truncation() {
        assert_fixed_eq(Fixed::units(2), Fixed::milli(2000));
        assert_fixed_eq(Fixed::new(2, 500), Fixed::milli(2500));
        assert_fixed_eq(
            Fixed::milli(1500).checked_div(Fixed::units(2)).unwrap(),
            Fixed::milli(750),
        );
        assert_fixed_eq(
            Fixed::milli(333).fixed_mul(Fixed::units(3)),
            Fixed::milli(999),
        );
    }

    #[test]
    fn value_expr_evaluates_constants_metrics_and_arithmetic() {
        let ctx = TestEvalContext::with_metrics(&[
            ("population", Fixed::units(13)),
            ("housing", Fixed::units(10)),
        ]);

        assert_fixed_eq(
            ValueExpr::Const(Fixed::milli(2500)).evaluate(&ctx).unwrap(),
            Fixed::milli(2500),
        );
        assert_fixed_eq(
            ValueExpr::Metric("population".into())
                .evaluate(&ctx)
                .unwrap(),
            Fixed::units(13),
        );
        assert_fixed_eq(
            ValueExpr::Add(vec![
                ValueExpr::Const(Fixed::units(1)),
                ValueExpr::Const(Fixed::units(2)),
                ValueExpr::Const(Fixed::units(3)),
            ])
            .evaluate(&ctx)
            .unwrap(),
            Fixed::units(6),
        );
        assert_fixed_eq(ValueExpr::Add(vec![]).evaluate(&ctx).unwrap(), Fixed::ZERO);
        assert_fixed_eq(
            ValueExpr::Sub(
                boxed_value(ValueExpr::Const(Fixed::units(7))),
                boxed_value(ValueExpr::Const(Fixed::units(2))),
            )
            .evaluate(&ctx)
            .unwrap(),
            Fixed::units(5),
        );
        assert_fixed_eq(
            ValueExpr::Mul(
                boxed_value(ValueExpr::Const(Fixed::units(4))),
                boxed_value(ValueExpr::Const(Fixed::units(3))),
            )
            .evaluate(&ctx)
            .unwrap(),
            Fixed::units(12),
        );
        assert_fixed_eq(
            ValueExpr::Div(
                boxed_value(ValueExpr::Const(Fixed::units(9))),
                boxed_value(ValueExpr::Const(Fixed::units(3))),
            )
            .evaluate(&ctx)
            .unwrap(),
            Fixed::units(3),
        );
    }

    #[test]
    fn value_expr_evaluates_min_max_clamp_and_summarized_inner() {
        let ctx = TestEvalContext::with_metrics(&[
            ("population", Fixed::units(13)),
            ("housing", Fixed::units(10)),
        ]);
        let excess = ValueExpr::Max(
            boxed_value(ValueExpr::Sub(
                boxed_value(ValueExpr::Metric("population".into())),
                boxed_value(ValueExpr::Metric("housing".into())),
            )),
            boxed_value(ValueExpr::Const(Fixed::ZERO)),
        );

        assert_fixed_eq(excess.evaluate(&ctx).unwrap(), Fixed::units(3));
        assert_fixed_eq(
            ValueExpr::Min(
                boxed_value(ValueExpr::Const(Fixed::units(4))),
                boxed_value(ValueExpr::Const(Fixed::units(9))),
            )
            .evaluate(&ctx)
            .unwrap(),
            Fixed::units(4),
        );
        assert_fixed_eq(
            ValueExpr::Clamp {
                value: boxed_value(ValueExpr::Const(Fixed::units(12))),
                min: boxed_value(ValueExpr::Const(Fixed::ZERO)),
                max: boxed_value(ValueExpr::Const(Fixed::units(10))),
            }
            .evaluate(&ctx)
            .unwrap(),
            Fixed::units(10),
        );
        assert_fixed_eq(
            ValueExpr::Summarized {
                label: "Overpopulation penalty".into(),
                mode: ExprDisplayMode::SummaryOnly,
                inner: boxed_value(ValueExpr::Mul(
                    boxed_value(excess),
                    boxed_value(ValueExpr::Const(Fixed::milli(-50))),
                )),
            }
            .evaluate(&ctx)
            .unwrap(),
            Fixed::milli(-150),
        );
    }

    #[test]
    fn value_expr_evaluates_host_functions_after_arguments() {
        let ctx = TestEvalContext::with_metrics(&[
            ("population", Fixed::units(13)),
            ("housing", Fixed::units(10)),
        ]);

        assert_fixed_eq(
            ValueExpr::HostFunction {
                id: "sum".into(),
                args: vec![
                    ValueExpr::Const(Fixed::units(1)),
                    ValueExpr::Const(Fixed::units(2)),
                ],
                display: HostExprDisplay::default(),
            }
            .evaluate(&ctx)
            .unwrap(),
            Fixed::units(3),
        );

        assert_fixed_eq(
            ValueExpr::HostFunction {
                id: "weighted_excess".into(),
                args: vec![
                    ValueExpr::Metric("population".into()),
                    ValueExpr::Metric("housing".into()),
                    ValueExpr::Const(Fixed::milli(-50)),
                ],
                display: HostExprDisplay::default(),
            }
            .evaluate(&ctx)
            .unwrap(),
            Fixed::milli(-150),
        );
    }

    #[test]
    fn value_expr_reports_unknown_inputs_and_division_by_zero() {
        let ctx = TestEvalContext::default();

        assert_eq!(
            ValueExpr::Metric("missing".into()).evaluate(&ctx),
            Err(ModificationEvalError::UnknownMetric("missing".into()))
        );
        assert_eq!(
            ValueExpr::HostFunction {
                id: "missing_fn".into(),
                args: vec![],
                display: HostExprDisplay::default(),
            }
            .evaluate(&ctx),
            Err(ModificationEvalError::UnknownHostFunction(
                "missing_fn".into()
            ))
        );
        assert_eq!(
            ValueExpr::Div(
                boxed_value(ValueExpr::Const(Fixed::units(1))),
                boxed_value(ValueExpr::Const(Fixed::ZERO))
            )
            .evaluate(&ctx),
            Err(ModificationEvalError::DivisionByZero)
        );
    }

    #[test]
    fn bool_expr_evaluates_boolean_combinators() {
        let ctx = TestEvalContext::default();

        assert!(BoolExpr::True.evaluate(&ctx).unwrap());
        assert!(!BoolExpr::False.evaluate(&ctx).unwrap());
        assert!(BoolExpr::All(vec![]).evaluate(&ctx).unwrap());
        assert!(!BoolExpr::Any(vec![]).evaluate(&ctx).unwrap());
        assert!(
            BoolExpr::All(vec![BoolExpr::True, BoolExpr::True])
                .evaluate(&ctx)
                .unwrap()
        );
        assert!(
            !BoolExpr::All(vec![BoolExpr::True, BoolExpr::False])
                .evaluate(&ctx)
                .unwrap()
        );
        assert!(
            BoolExpr::Any(vec![BoolExpr::False, BoolExpr::True])
                .evaluate(&ctx)
                .unwrap()
        );
        assert!(
            BoolExpr::OneOf(vec![BoolExpr::False, BoolExpr::True, BoolExpr::False])
                .evaluate(&ctx)
                .unwrap()
        );
        assert!(
            !BoolExpr::OneOf(vec![BoolExpr::True, BoolExpr::True])
                .evaluate(&ctx)
                .unwrap()
        );
        assert!(
            BoolExpr::Not(boxed_bool(BoolExpr::False))
                .evaluate(&ctx)
                .unwrap()
        );
    }

    #[test]
    fn bool_expr_evaluates_all_compare_ops() {
        let ctx = TestEvalContext::default();

        for (op, expected) in [
            (CompareOp::Eq, true),
            (CompareOp::Ne, false),
            (CompareOp::Lt, false),
            (CompareOp::Lte, true),
            (CompareOp::Gt, false),
            (CompareOp::Gte, true),
        ] {
            assert_eq!(
                BoolExpr::Compare {
                    left: ValueExpr::Const(Fixed::units(2)),
                    op,
                    right: ValueExpr::Const(Fixed::units(2)),
                }
                .evaluate(&ctx)
                .unwrap(),
                expected
            );
        }

        assert!(
            BoolExpr::Compare {
                left: ValueExpr::Const(Fixed::units(3)),
                op: CompareOp::Gt,
                right: ValueExpr::Const(Fixed::units(2)),
            }
            .evaluate(&ctx)
            .unwrap()
        );
    }

    #[test]
    fn bool_expr_evaluates_summarized_and_host_predicates() {
        let ctx = TestEvalContext::with_metrics(&[("value", Fixed::units(5))]);

        assert!(
            BoolExpr::Summarized {
                label: "Visible summary".into(),
                mode: ExprDisplayMode::SummaryOnly,
                inner: boxed_bool(BoolExpr::Compare {
                    left: ValueExpr::Metric("value".into()),
                    op: CompareOp::Gte,
                    right: ValueExpr::Const(Fixed::units(5)),
                }),
            }
            .evaluate(&ctx)
            .unwrap()
        );

        assert!(
            BoolExpr::HostPredicate {
                id: "positive".into(),
                args: vec![
                    ValueExpr::Const(Fixed::units(1)),
                    ValueExpr::Metric("value".into())
                ],
                display: HostExprDisplay::default(),
            }
            .evaluate(&ctx)
            .unwrap()
        );

        assert!(
            BoolExpr::HostPredicate {
                id: "between".into(),
                args: vec![
                    ValueExpr::Metric("value".into()),
                    ValueExpr::Const(Fixed::units(1)),
                    ValueExpr::Const(Fixed::units(10)),
                ],
                display: HostExprDisplay::default(),
            }
            .evaluate(&ctx)
            .unwrap()
        );
    }

    #[test]
    fn bool_expr_reports_unknown_host_predicate_and_value_errors() {
        let ctx = TestEvalContext::default();

        assert_eq!(
            BoolExpr::HostPredicate {
                id: "missing_predicate".into(),
                args: vec![],
                display: HostExprDisplay::default(),
            }
            .evaluate(&ctx),
            Err(ModificationEvalError::UnknownHostPredicate(
                "missing_predicate".into()
            ))
        );

        assert_eq!(
            BoolExpr::Compare {
                left: ValueExpr::Metric("missing".into()),
                op: CompareOp::Eq,
                right: ValueExpr::Const(Fixed::ZERO),
            }
            .evaluate(&ctx),
            Err(ModificationEvalError::UnknownMetric("missing".into()))
        );
    }

    #[test]
    fn modifier_projection_evaluates_all_channels_and_defaults_missing_channels() {
        let ctx = TestEvalContext::with_metrics(&[("excess_population", Fixed::units(3))]);

        let evaluated = ModifierProjection {
            target: "colony.stability".into(),
            base_add: None,
            multiplier: Some(ValueExpr::Mul(
                boxed_value(ValueExpr::Metric("excess_population".into())),
                boxed_value(ValueExpr::Const(Fixed::milli(-50))),
            )),
            add: Some(ValueExpr::Const(Fixed::units(-1))),
        }
        .evaluate(&ctx)
        .unwrap();

        assert_eq!(evaluated.target, "colony.stability");
        assert_fixed_eq(evaluated.base_add, Fixed::ZERO);
        assert_fixed_eq(evaluated.multiplier, Fixed::milli(-150));
        assert_fixed_eq(evaluated.add, Fixed::units(-1));
    }

    #[test]
    fn modification_rule_evaluates_when_absent_or_true() {
        let ctx = TestEvalContext::with_metrics(&[
            ("population", Fixed::units(13)),
            ("housing", Fixed::units(10)),
        ]);

        let rule = ModificationRule {
            id: "colony.overpopulation".into(),
            label: "Overpopulation".into(),
            source: source_ref(),
            when: Some(BoolExpr::Compare {
                left: ValueExpr::Metric("population".into()),
                op: CompareOp::Gt,
                right: ValueExpr::Metric("housing".into()),
            }),
            tags: vec!["overpopulated".into()],
            projections: vec![ModifierProjection {
                target: "colony.stability".into(),
                base_add: None,
                multiplier: Some(ValueExpr::Mul(
                    boxed_value(ValueExpr::Max(
                        boxed_value(ValueExpr::Sub(
                            boxed_value(ValueExpr::Metric("population".into())),
                            boxed_value(ValueExpr::Metric("housing".into())),
                        )),
                        boxed_value(ValueExpr::Const(Fixed::ZERO)),
                    )),
                    boxed_value(ValueExpr::Const(Fixed::milli(-50))),
                )),
                add: None,
            }],
        };

        let evaluated = rule.evaluate(&ctx).unwrap().unwrap();
        assert_eq!(evaluated.id, "colony.overpopulation");
        assert_eq!(evaluated.label, "Overpopulation");
        assert_eq!(evaluated.source, source_ref());
        assert_eq!(evaluated.tags, vec!["overpopulated"]);
        assert_eq!(evaluated.projections.len(), 1);
        assert_eq!(evaluated.projections[0].target, "colony.stability");
        assert_fixed_eq(evaluated.projections[0].base_add, Fixed::ZERO);
        assert_fixed_eq(evaluated.projections[0].multiplier, Fixed::milli(-150));
        assert_fixed_eq(evaluated.projections[0].add, Fixed::ZERO);

        let unconditional = ModificationRule { when: None, ..rule };
        assert!(unconditional.evaluate(&ctx).unwrap().is_some());
    }

    #[test]
    fn modification_rule_returns_none_when_condition_is_false() {
        let ctx = TestEvalContext::with_metrics(&[
            ("population", Fixed::units(8)),
            ("housing", Fixed::units(10)),
        ]);
        let rule = ModificationRule {
            id: "colony.overpopulation".into(),
            label: "Overpopulation".into(),
            source: source_ref(),
            when: Some(BoolExpr::Compare {
                left: ValueExpr::Metric("population".into()),
                op: CompareOp::Gt,
                right: ValueExpr::Metric("housing".into()),
            }),
            tags: vec!["overpopulated".into()],
            projections: vec![ModifierProjection {
                target: "colony.stability".into(),
                base_add: None,
                multiplier: Some(ValueExpr::Metric("missing_metric".into())),
                add: None,
            }],
        };

        assert_eq!(rule.evaluate(&ctx), Ok(None));
    }

    #[test]
    fn modification_rule_propagates_condition_and_projection_errors() {
        let ctx = TestEvalContext::default();
        let condition_error = ModificationRule {
            id: "bad_condition".into(),
            label: "Bad Condition".into(),
            source: source_ref(),
            when: Some(BoolExpr::Compare {
                left: ValueExpr::Metric("missing_condition_metric".into()),
                op: CompareOp::Gt,
                right: ValueExpr::Const(Fixed::ZERO),
            }),
            tags: vec![],
            projections: vec![],
        };
        assert_eq!(
            condition_error.evaluate(&ctx),
            Err(ModificationEvalError::UnknownMetric(
                "missing_condition_metric".into()
            ))
        );

        let projection_error = ModificationRule {
            id: "bad_projection".into(),
            label: "Bad Projection".into(),
            source: source_ref(),
            when: Some(BoolExpr::True),
            tags: vec![],
            projections: vec![ModifierProjection {
                target: "colony.stability".into(),
                base_add: Some(ValueExpr::Metric("missing_projection_metric".into())),
                multiplier: None,
                add: None,
            }],
        };
        assert_eq!(
            projection_error.evaluate(&ctx),
            Err(ModificationEvalError::UnknownMetric(
                "missing_projection_metric".into()
            ))
        );
    }
}
