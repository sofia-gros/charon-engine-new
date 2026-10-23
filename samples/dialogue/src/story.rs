//! This game's story and keys. The dialogue plugin contains no sample content.
use charon_dialogue::{Choice, Dialogue, Node};
use std::sync::Arc;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    Intro,
    Choice,
    River,
    Hill,
}
pub fn nodes() -> Vec<Node<Page>> {
    vec![
    Node {key: Page::Intro,text: "夕暮れの分かれ道で、あなたは旅の案内人と出会いました。\n遠くの街には、まだ明かりが灯っています。".into(),next: Some(Page::Choice),choices: Arc::from([])},
    Node {key: Page::Choice,text: "街へ向かう道は二つあります。\nあなたは、どちらの道を選びますか？".into(),next: None,choices: Arc::from([Choice {label: "川沿いの道へ".into(),target: Page::River},Choice {label: "丘を越える道へ".into(),target: Page::Hill}])},
    Node {key: Page::River,text: "川面が月の光を映して、銀色に輝いています。\nあなたは夜明け前に、街へたどり着きました。".into(),next: None,choices: Arc::from([])},
    Node {key: Page::Hill,text: "丘の上から、街の明かりが見えます。\nここから、あなたの新しい旅が始まります。".into(),next: None,choices: Arc::from([])},
]
}
pub fn build() -> Result<Dialogue<Page>, &'static str> {
    Dialogue::new(nodes(), Page::Intro)
}
