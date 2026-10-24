//! Bounded platformer route search using the same swept controller as gameplay.
//! Returns the next six-tick action; the game owns scheduling and replanning.
use charon_platformer::Body;
use std::{
    cmp::Reverse,
    collections::{BTreeSet, BinaryHeap},
};
pub const MAX_EXPANSIONS: usize = 128;
const MAX_NODES: usize = 641;
#[derive(Clone, Copy, Debug, Default)]
pub struct Control {
    pub direction: i8,
    pub jump: bool,
}
pub struct Plan {
    pub control: Control,
    pub expansions: usize,
    pub reached: bool,
}
struct Node {
    body: Body,
    cost: usize,
    first: Control,
}
fn key(body: Body) -> (i32, i32, i32, bool) {
    (
        (body.x() / 2.).floor() as i32,
        (body.y() / 2.).floor() as i32,
        (body.vertical_velocity() / 20.).round() as i32,
        body.grounded,
    )
}
fn distance(body: Body, target: [f32; 2]) -> f32 {
    (body.x() - target[0]).abs() / 1.5 + (body.y() - target[1]).abs() / 3.
}
pub fn plan(start: Body, target: [f32; 2], solid: impl Fn(i32, i32) -> bool) -> Plan {
    if !target.iter().all(|v| v.is_finite()) {
        return Plan {
            control: Control::default(),
            expansions: 0,
            reached: false,
        };
    }
    let mut nodes = vec![Node {
        body: start,
        cost: 0,
        first: Control::default(),
    }];
    let mut open = BinaryHeap::from([Reverse(((distance(start, target) * 100.) as i64, 0_usize))]);
    let mut visited = BTreeSet::new();
    let mut best = (distance(start, target), 0);
    let mut expansions = 0;
    while let Some(Reverse((_, index))) = open.pop() {
        let body = nodes[index].body;
        let cost = nodes[index].cost;
        let first = nodes[index].first;
        if !visited.insert(key(body)) {
            continue;
        }
        if (body.x() - target[0]).abs() < 12. && (body.y() - target[1]).abs() < 6. {
            return Plan {
                control: first,
                expansions,
                reached: true,
            };
        }
        if expansions == MAX_EXPANSIONS {
            break;
        }
        expansions += 1;
        for control in [
            Control {
                direction: -1,
                jump: false,
            },
            Control {
                direction: 1,
                jump: false,
            },
            Control {
                direction: 0,
                jump: true,
            },
            Control {
                direction: -1,
                jump: true,
            },
            Control {
                direction: 1,
                jump: true,
            },
        ] {
            if nodes.len() >= MAX_NODES {
                break;
            }
            let mut next = body;
            for step in 0..6 {
                next.step(control.direction, control.jump && step == 0, &solid);
                if next.embedded {
                    break;
                }
            }
            if next.embedded
                || visited.contains(&key(next))
                || (next.x() - start.x()).abs() > 160.
                || (next.y() - start.y()).abs() > 128.
            {
                continue;
            }
            let first = if index == 0 { control } else { first };
            let distance = distance(next, target);
            let new_index = nodes.len();
            if distance < best.0 {
                best = (distance, new_index);
            }
            nodes.push(Node {
                body: next,
                cost: cost + 6,
                first,
            });
            open.push(Reverse((
                (((cost + 6) as f32 + distance) * 100.) as i64,
                new_index,
            )));
        }
    }
    Plan {
        control: nodes[best.1].first,
        expansions,
        reached: false,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routes_use_intermediate_ledge_and_never_pass_through_terrain() {
        let solid = |x, y| y >= 100 || ((60..76).contains(&x) && y == 80) || (x < 45 && y >= 60);
        let mut body = Body::new(82., 88., 7., 12.);
        body.restore_motion(0., true, false).unwrap();
        let mut control = Control::default();
        let mut reached = false;
        for tick in 0..360 {
            if tick % 6 == 0 {
                let p = plan(body, [25., 48.], solid);
                assert!(p.expansions <= MAX_EXPANSIONS);
                control = p.control;
            }
            body.step(control.direction, control.jump && tick % 6 == 0, solid);
            assert!(!body.embedded);
            if body.x() < 40. && body.y() <= 49. {
                reached = true;
                break;
            }
        }
        assert!(reached, "could not use intermediate ledge: {body:?}");
    }
    #[test]
    fn sealed_wall_has_a_finite_search_and_no_teleport() {
        let solid = |x, y| y >= 100 || ((50..55).contains(&x) && y < 100);
        let mut body = Body::new(80., 88., 7., 12.);
        body.restore_motion(0., true, false).unwrap();
        for _ in 0..20 {
            let p = plan(body, [20., 88.], solid);
            assert!(p.expansions <= MAX_EXPANSIONS);
            assert!(!p.reached);
            for tick in 0..6 {
                body.step(p.control.direction, p.control.jump && tick == 0, solid);
            }
            assert!(body.x() >= 55. && !body.embedded);
        }
    }
}
