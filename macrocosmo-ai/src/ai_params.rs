//! `ai_params` フィールド (\[String -> f64\] 汎用 key-value) を解釈するための trait。
//!
//! engine 側の `FactionDefinition` は `ai_params` を generic な key-value として
//! 持つだけで、AI 用の具体的な key 名 (aggressiveness, expansionism, ...) は
//! 認識しない。`AiParamsExt` trait を通じて AI crate が解釈する。
//!
//! 本モジュールは 0.3.0 AI 実装時に使い始める想定。#189 umbrella 参照。

/// AI パラメータを解釈するための拡張 trait。
/// FactionDefinition や FactionTypeDefinition に実装する予定。
///
/// 現段階では trait 定義のみ。具体的な実装は #189 以降のサブ issue で追加する。
pub trait AiParamsExt {
    /// 指定 key の f64 値を取得。未指定なら `default`。
    fn ai_param_f64(&self, key: &str, default: f64) -> f64;

    /// よく使われる性格パラメータのショートカット (#189 参照)。
    fn aggressiveness(&self) -> f64 {
        self.ai_param_f64("aggressiveness", 0.5)
    }

    fn expansionism(&self) -> f64 {
        self.ai_param_f64("expansionism", 0.5)
    }

    fn defensive_bias(&self) -> f64 {
        self.ai_param_f64("defensive_bias", 0.5)
    }

    fn objective_persistence(&self) -> f64 {
        self.ai_param_f64("objective_persistence", 0.6)
    }

    fn approach_flexibility(&self) -> f64 {
        self.ai_param_f64("approach_flexibility", 0.6)
    }

    fn delegation_autonomy(&self) -> f64 {
        self.ai_param_f64("delegation_autonomy", 0.6)
    }

    fn intent_staleness_tolerance(&self) -> f64 {
        self.ai_param_f64("intent_staleness_tolerance", 20.0)
    }
}
