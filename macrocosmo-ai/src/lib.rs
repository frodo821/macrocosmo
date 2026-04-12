//! macrocosmo-ai — 疎結合 AI クレート。
//!
//! macrocosmo engine が提供する FactionRelations / KnowledgeStore / Ship 等を
//! 読み取り専用で参照し、CommandQueue / PendingDiplomaticAction / ScopedFlags
//! への書き込みのみを行う。
//!
//! 境界ルール:
//! - 他 faction の真値にはアクセスしない (KnowledgeStore 経由のみ)
//! - Lua Engine には間接アクセス (engine の提供する Command 型経由)
//!
//! 初期段階では空の Plugin のみ提供。個別 AI 機能 (#189-#194) は段階的に
//! ここに実装される。

use bevy::prelude::*;

/// AI 基盤 Plugin。
///
/// 登録方法: 現状の macrocosmo クレートは lib + bin の単一クレートのため、
/// macrocosmo → macrocosmo-ai の循環依存を避けて **本プラグインは bin から
/// 登録されない**。AI 実装時 (#189 以降) に以下のどちらかで統合予定:
///
/// 1. `macrocosmo-bin` を別 crate として切り出し、両者を依存する
///    (案 B、0.3.0 完成後)
/// 2. macrocosmo engine に AiPlugin 用の hook trait を定義し、macrocosmo-ai
///    がそれを実装する形 (案 C)
///
/// いずれも #189 AI umbrella 実装時に決定。現段階では空プラグインとして用意。
pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, _app: &mut App) {
        // #189 以降の AI system がここに登録される。
    }
}

pub mod ai_params;
