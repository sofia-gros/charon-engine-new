//! Small typed dialogue state machine. No renderer, sand, or window dependency.
use charon_runtime::Simulation;

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Advance,
    Choose(usize),
    Reset,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Intro,
    Choice,
    River,
    Hill,
}
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub page: Page,
    pub text: &'static str,
    pub prompt: &'static str,
}
#[derive(Default)]
pub struct Dialogue {
    page: Page,
}
impl Simulation for Dialogue {
    type Command = Command;
    type Frame = Frame;
    fn step(&mut self, commands: Vec<Command>) -> Frame {
        for command in commands {
            self.page = match (self.page, command) {
                (_, Command::Reset) => Page::Intro,
                (Page::Intro, Command::Advance) => Page::Choice,
                (Page::Choice, Command::Choose(0)) => Page::River,
                (Page::Choice, Command::Choose(1)) => Page::Hill,
                (page, _) => page,
            };
        }
        let (text, prompt) = match self.page {
            Page::Intro => (
                "THE LAST LIGHT FADES.\nA MESSENGER WAITS AT THE CROSSROADS.",
                "SPACE OR CLICK TO CONTINUE",
            ),
            Page::Choice => (
                "TWO PATHS LEAD TO THE CITY.\nWHERE WILL YOU GO?",
                "1: FOLLOW THE RIVER\n2: CROSS THE HILL",
            ),
            Page::River => (
                "THE RIVER CARRIES A SILVER LIGHT.\nYOU REACH THE CITY BEFORE DAWN.",
                "R: RESTART",
            ),
            Page::Hill => (
                "FROM THE HILL YOU SEE THE CITY.\nA NEW JOURNEY BEGINS.",
                "R: RESTART",
            ),
        };
        Frame {
            page: self.page,
            text,
            prompt,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guarded_choices_branches_and_reset() {
        let mut story = Dialogue::default();
        assert_eq!(story.step(vec![Command::Choose(0)]).page, Page::Intro);
        for (choice, expected) in [(0, Page::River), (1, Page::Hill)] {
            story.step(vec![Command::Reset, Command::Advance]);
            assert_eq!(story.step(vec![Command::Choose(99)]).page, Page::Choice);
            let frame = story.step(vec![Command::Choose(choice), Command::Advance]);
            assert_eq!(frame.page, expected);
            assert_eq!(story.step(vec![Command::Reset]).page, Page::Intro);
            assert_eq!(frame.page, expected); // Published frames remain unchanged.
        }
    }
}
