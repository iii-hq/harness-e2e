//! A deterministic world with rules the agent can only learn by acting.
//!
//! The runner owns the state, so success is what the world recorded, never
//! what the session claimed. Every level permutes which action does what, so
//! the mapping has to be rediscovered rather than carried over.

use serde_json::{json, Value};

pub(in crate::scenarios) const ACTIONS: [&str; 4] = ["A", "B", "C", "D"];
pub(in crate::scenarios) const SIZE: i64 = 5;
pub(in crate::scenarios) const LEVELS: usize = 3;
/// Per level, the effect of A, B, C, D in order. Permuted so level two and
/// three cannot be solved with level one's mapping.
const MAPPINGS: [[Effect; 4]; LEVELS] = [
    [Effect::Right, Effect::Down, Effect::Left, Effect::Up],
    [Effect::Up, Effect::Left, Effect::Right, Effect::Down],
    [Effect::Left, Effect::Up, Effect::Down, Effect::Right],
];
type Cell = (i64, i64);

/// Where a level starts, the key it may require, and where it ends. A level
/// with a key does not open until the key has been stood on.
struct Layout {
    start: Cell,
    key: Option<Cell>,
    goal: Cell,
}

const LAYOUTS: [Layout; LEVELS] = [
    Layout {
        start: (0, 0),
        key: None,
        goal: (3, 2),
    },
    Layout {
        start: (2, 4),
        key: Some((0, 1)),
        goal: (4, 0),
    },
    Layout {
        start: (4, 4),
        key: Some((1, 3)),
        goal: (0, 0),
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    Up,
    Down,
    Left,
    Right,
}

impl Effect {
    fn apply(self, (x, y): Cell) -> Cell {
        let (dx, dy) = match self {
            Self::Up => (0, -1),
            Self::Down => (0, 1),
            Self::Left => (-1, 0),
            Self::Right => (1, 0),
        };
        ((x + dx).clamp(0, SIZE - 1), (y + dy).clamp(0, SIZE - 1))
    }
}

pub(in crate::scenarios) struct World {
    level: usize,
    position: Cell,
    carrying_key: bool,
    actions_used: usize,
    invalid_actions: usize,
    actions_per_level: Vec<usize>,
    solved: usize,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> Self {
        Self {
            level: 0,
            position: LAYOUTS[0].start,
            carrying_key: false,
            actions_used: 0,
            invalid_actions: 0,
            actions_per_level: Vec::new(),
            solved: 0,
        }
    }

    pub fn solved(&self) -> usize {
        self.solved
    }

    pub fn actions_used(&self) -> usize {
        self.actions_used
    }

    pub fn invalid_actions(&self) -> usize {
        self.invalid_actions
    }

    /// Actions spent on the levels that were actually solved: the efficiency
    /// number, unpolluted by a level the session never finished.
    pub fn actions_per_solved_level(&self) -> Option<f64> {
        (!self.actions_per_level.is_empty()).then(|| {
            let total: usize = self.actions_per_level.iter().sum();
            total as f64 / self.actions_per_level.len() as f64
        })
    }

    pub fn observation(&self) -> Value {
        let Layout { key, goal, .. } = LAYOUTS[self.level.min(LEVELS - 1)];
        json!({
            "level": self.level + 1,
            "levels_total": LEVELS,
            "position": [self.position.0, self.position.1],
            "goal": [goal.0, goal.1],
            "key": key.map(|(x, y)| json!([x, y])),
            "carrying_key": self.carrying_key,
            "actions_used": self.actions_used,
            "levels_solved": self.solved,
            "finished": self.finished(),
        })
    }

    pub fn finished(&self) -> bool {
        self.solved >= LEVELS
    }

    /// Apply one action. Unknown labels are counted and change nothing, so a
    /// malformed call can never advance the world by accident.
    pub fn act(&mut self, action: &str) -> Value {
        if self.finished() {
            return self.observation();
        }
        let Some(index) = ACTIONS.iter().position(|known| *known == action) else {
            self.invalid_actions += 1;
            return json!({ "error": "unknown action", "observation": self.observation() });
        };

        self.actions_used += 1;
        let level = self.level;
        let Layout { key, goal, .. } = LAYOUTS[level];
        self.position = MAPPINGS[level][index].apply(self.position);

        if key.is_some_and(|key| key == self.position) {
            self.carrying_key = true;
        }
        let reached = self.position == goal;
        let unlocked = key.is_none() || self.carrying_key;
        if reached && unlocked {
            self.solved += 1;
            let spent = self.actions_used - self.actions_per_level.iter().sum::<usize>();
            self.actions_per_level.push(spent);
            if self.level + 1 < LEVELS {
                self.level += 1;
                self.position = LAYOUTS[self.level].start;
                self.carrying_key = false;
            }
        }
        self.observation()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive a level by its true mapping, which the agent has to discover.
    fn walk(world: &mut World, effect: Effect) {
        let level = world.level;
        let index = MAPPINGS[level]
            .iter()
            .position(|candidate| *candidate == effect)
            .expect("every effect appears once per level");
        world.act(ACTIONS[index]);
    }

    #[test]
    fn a_level_without_a_key_completes_by_reaching_the_goal() {
        let mut world = World::new();
        for _ in 0..3 {
            walk(&mut world, Effect::Right);
        }
        for _ in 0..2 {
            walk(&mut world, Effect::Down);
        }
        assert_eq!(world.solved(), 1);
        assert_eq!(world.observation()["level"], 2);
    }

    #[test]
    fn the_goal_stays_shut_until_the_key_is_collected() {
        let mut world = World::new();
        world.level = 1;
        world.position = LAYOUTS[1].start;
        for _ in 0..2 {
            walk(&mut world, Effect::Right);
        }
        for _ in 0..4 {
            walk(&mut world, Effect::Up);
        }
        assert_eq!(world.solved(), 0, "reached the goal without the key");
    }

    #[test]
    fn an_unknown_action_moves_nothing() {
        let mut world = World::new();
        let before = world.observation();
        world.act("Z");
        assert_eq!(world.invalid_actions(), 1);
        assert_eq!(world.actions_used(), 0);
        assert_eq!(world.observation()["position"], before["position"]);
    }

    #[test]
    fn each_level_permutes_the_mapping() {
        for (level, mapping) in MAPPINGS.iter().enumerate().skip(1) {
            assert_ne!(&MAPPINGS[0], mapping, "level {level}");
        }
        for mapping in MAPPINGS {
            let mut seen = mapping.to_vec();
            seen.sort_by_key(|effect| format!("{effect:?}"));
            seen.dedup();
            assert_eq!(seen.len(), ACTIONS.len(), "an effect is unreachable");
        }
    }

    #[test]
    fn the_world_stops_after_the_last_level() {
        let mut world = World::new();
        world.solved = LEVELS;
        world.act("A");
        assert_eq!(world.actions_used(), 0);
        assert!(world.finished());
    }
}
