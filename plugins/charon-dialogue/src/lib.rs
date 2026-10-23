//! Reusable dialogue transitions. Story content and typed keys belong to the game.
use charon_runtime::Simulation;
use std::{collections::HashMap, hash::Hash, sync::Arc};

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Advance,
    Choose(usize),
    Reset,
}

#[derive(Clone, Debug)]
pub struct Choice<K> {
    pub label: Arc<str>,
    pub target: K,
}
#[derive(Clone, Debug)]
pub struct Node<K> {
    pub key: K,
    pub text: Arc<str>,
    pub next: Option<K>,
    pub choices: Arc<[Choice<K>]>,
}
#[derive(Clone, Debug)]
pub struct Frame<K> {
    pub page: K,
    pub text: Arc<str>,
    pub can_advance: bool,
    pub choices: Arc<[Choice<K>]>,
}
pub struct Dialogue<K> {
    nodes: HashMap<K, Node<K>>,
    entry: K,
    current: K,
}
impl<K: Copy + Eq + Hash> Dialogue<K> {
    pub fn new(nodes: Vec<Node<K>>, entry: K) -> Result<Self, &'static str> {
        let count = nodes.len();
        let nodes: HashMap<_, _> = nodes.into_iter().map(|node| (node.key, node)).collect();
        if nodes.len() != count {
            return Err("duplicate dialogue key");
        }
        if !nodes.contains_key(&entry) {
            return Err("missing dialogue entry");
        }
        for node in nodes.values() {
            if node.next.is_some() && !node.choices.is_empty() {
                return Err("dialogue node cannot mix continuation and choices");
            }
            if node
                .next
                .into_iter()
                .chain(node.choices.iter().map(|choice| choice.target))
                .any(|key| !nodes.contains_key(&key))
            {
                return Err("missing dialogue target");
            }
            if node
                .choices
                .iter()
                .any(|choice| choice.label.trim().is_empty())
            {
                return Err("empty dialogue choice label");
            }
        }
        Ok(Self {
            nodes,
            entry,
            current: entry,
        })
    }
}
impl<K: Copy + Eq + Hash + Send + Sync + 'static> Simulation for Dialogue<K> {
    type Command = Command;
    type Frame = Frame<K>;
    fn step(&mut self, commands: Vec<Command>) -> Frame<K> {
        for command in commands {
            let node = &self.nodes[&self.current];
            self.current = match command {
                Command::Reset => self.entry,
                Command::Advance => node.next.unwrap_or(self.current),
                Command::Choose(index) => node
                    .choices
                    .get(index)
                    .map_or(self.current, |choice| choice.target),
            };
        }
        let node = &self.nodes[&self.current];
        Frame {
            page: self.current,
            text: Arc::clone(&node.text),
            can_advance: node.next.is_some(),
            choices: Arc::clone(&node.choices),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn nodes() -> Vec<Node<u8>> {
        vec![
            Node {
                key: 0,
                text: "ゲーム側の文章".into(),
                next: Some(1),
                choices: Arc::from([]),
            },
            Node {
                key: 1,
                text: "選択".into(),
                next: None,
                choices: Arc::from([
                    Choice {
                        label: "左".into(),
                        target: 2,
                    },
                    Choice {
                        label: "右".into(),
                        target: 3,
                    },
                ]),
            },
            Node {
                key: 2,
                text: "左の結末".into(),
                next: None,
                choices: Arc::from([]),
            },
            Node {
                key: 3,
                text: "右の結末".into(),
                next: None,
                choices: Arc::from([]),
            },
        ]
    }
    #[test]
    fn game_owned_graph_guards_choices_and_preserves_published_frames() {
        let mut story = Dialogue::new(nodes(), 0).unwrap();
        assert_eq!(story.step(vec![Command::Choose(0)]).page, 0);
        for (choice, target) in [(0, 2), (1, 3)] {
            story.step(vec![Command::Reset, Command::Advance]);
            assert_eq!(story.step(vec![Command::Choose(99)]).page, 1);
            let frame = story.step(vec![Command::Choose(choice), Command::Advance]);
            assert_eq!(frame.page, target);
            story.step(vec![Command::Reset]);
            assert_eq!(frame.page, target);
        }
    }
    #[test]
    fn invalid_story_targets_and_ambiguous_nodes_fail_before_play() {
        assert!(Dialogue::new(nodes(), 99).is_err());
        let mut duplicate = nodes();
        duplicate.push(duplicate[0].clone());
        assert!(Dialogue::new(duplicate, 0).is_err());
        let mut missing = nodes();
        missing[0].next = Some(99);
        assert!(Dialogue::new(missing, 0).is_err());
        let mut mixed = nodes();
        mixed[1].next = Some(0);
        assert!(Dialogue::new(mixed, 0).is_err());
    }
}
