//! the higher level application
//!
//! this module mostly handles
//! 1. the main TUI loop
//! 1. the rendering
//! 1. the keybindings
//! 1. the internal state of the application
use anyhow::Result;
use console::Key;
use ratatui::{prelude::CrosstermBackend, Terminal};

use nu_protocol::{
    ast::{CellPath, PathMember},
    ShellError, Span, Value,
};

use crate::edit::Editor;

use super::navigation::Direction;
use super::{config::Config, navigation, tui};

/// the mode in which the application is
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Mode {
    /// the NORMAL mode is the *navigation* mode, where the user can move around in the data
    Normal,
    /// the INSERT mode lets the user edit cells of the structured data
    Insert,
    /// the PEEKING mode lets the user *peek* data out of the application, to be reused later
    Peeking,
    /// TODO: documentation
    Bottom,
}

impl Default for Mode {
    fn default() -> Self {
        Self::Normal
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let repr = match self {
            Self::Normal => "NORMAL",
            Self::Insert => "INSERT",
            Self::Peeking => "PEEKING",
            Self::Bottom => "BOTTOM",
        };
        write!(f, "{}", repr)
    }
}

/// the complete state of the application
pub(super) struct App {
    /// the full current path in the data
    pub cell_path: CellPath,
    /// the current [`Mode`]
    pub mode: Mode,
    /// the editor to modify the cells of the data
    pub editor: Editor,
}

impl Default for App {
    fn default() -> Self {
        Self {
            cell_path: CellPath { members: vec![] },
            mode: Mode::default(),
            editor: Editor::default(),
        }
    }
}

impl App {
    pub(super) fn from_value(value: &Value) -> Self {
        let mut app = Self::default();
        match value {
            Value::List { vals, .. } => app.cell_path.members.push(PathMember::Int {
                val: 0,
                span: Span::unknown(),
                optional: vals.is_empty(),
            }),
            Value::Record { cols, .. } => app.cell_path.members.push(PathMember::String {
                val: cols.get(0).unwrap_or(&"".to_string()).into(),
                span: Span::unknown(),
                optional: cols.is_empty(),
            }),
            _ => {}
        }

        app
    }

    /// TODO: documentation
    pub(super) fn is_at_bottom(&self) -> bool {
        matches!(self.mode, Mode::Bottom)
    }

    /// TODO: documentation
    pub(super) fn hit_bottom(&mut self) {
        self.mode = Mode::Bottom;
    }

    fn enter_editor(&mut self, value: &Value) {
        self.mode = Mode::Insert;
        self.editor = Editor::from_value(value);
    }
}

/// the result of a state transition
#[derive(Debug, PartialEq)]
enum TransitionResult {
    Quit,
    Continue,
    Return(Value),
    Edit(Value),
    Error(String),
}

impl TransitionResult {
    #[cfg(test)]
    fn is_quit(&self) -> bool {
        matches!(self, Self::Quit | Self::Return(_))
    }
}

/// run the application
///
/// this function
/// 1. creates the initial [`App`]
/// 1. runs the main application loop
///
/// the application loop
/// 1. renders the TUI with [`tui`]
/// 1. reads the user's input keys and transition the [`App`] accordingly
pub(super) fn run(
    terminal: &mut Terminal<CrosstermBackend<console::Term>>,
    input: &Value,
    config: &Config,
) -> Result<Value> {
    let mut app = App::from_value(input);
    let mut value = input.clone();

    loop {
        if app.mode == Mode::Insert {
            app.editor
                .set_width(terminal.size().unwrap().width as usize)
        }

        terminal.draw(|frame| tui::render_ui(frame, &value, &app, config, None))?;

        let key = console::Term::stderr().read_key()?;
        match transition_state(&key, config, &mut app, &value)? {
            TransitionResult::Quit => break,
            TransitionResult::Continue => {}
            TransitionResult::Edit(val) => {
                value = crate::nu::value::mutate_value_cell(&value, &app.cell_path, &val)
            }
            TransitionResult::Error(error) => {
                terminal.draw(|frame| tui::render_ui(frame, &value, &app, config, Some(&error)))?;
                let _ = console::Term::stderr().read_key()?;
            }
            TransitionResult::Return(value) => return Ok(value),
        }
    }
    Ok(Value::nothing(Span::unknown()))
}

/// perform the state transition based on the key pressed and the previous state
#[allow(clippy::collapsible_if)]
fn transition_state(
    key: &Key,
    config: &Config,
    app: &mut App,
    value: &Value,
) -> Result<TransitionResult, ShellError> {
    if key == &config.keybindings.quit {
        if app.mode != Mode::Insert {
            return Ok(TransitionResult::Quit);
        }
    } else if key == &config.keybindings.insert {
        if app.mode == Mode::Normal {
            let value = &value
                .clone()
                .follow_cell_path(&app.cell_path.members, false)
                .unwrap();

            match value {
                Value::String { .. } => {
                    app.enter_editor(value);
                    return Ok(TransitionResult::Continue);
                }
                // TODO: support more diverse cell edition
                x => {
                    return Ok(TransitionResult::Error(format!(
                        "can only edit string cells, found {}",
                        x.get_type()
                    )))
                }
            }
        }
    } else if key == &config.keybindings.normal {
        if app.mode == Mode::Insert {
            app.mode = Mode::Normal;
            return Ok(TransitionResult::Continue);
        }
    } else if key == &config.keybindings.navigation.down {
        if app.mode == Mode::Normal {
            navigation::go_up_or_down_in_data(app, value, Direction::Down);
            return Ok(TransitionResult::Continue);
        }
    } else if key == &config.keybindings.navigation.up {
        if app.mode == Mode::Normal {
            navigation::go_up_or_down_in_data(app, value, Direction::Up);
            return Ok(TransitionResult::Continue);
        }
    } else if key == &config.keybindings.navigation.right {
        if app.mode == Mode::Normal {
            navigation::go_deeper_in_data(app, value);
            return Ok(TransitionResult::Continue);
        }
    } else if key == &config.keybindings.navigation.left {
        if app.mode == Mode::Normal {
            navigation::go_back_in_data(app);
            return Ok(TransitionResult::Continue);
        } else if app.is_at_bottom() {
            app.mode = Mode::Normal;
            return Ok(TransitionResult::Continue);
        }
    } else if key == &config.keybindings.peek {
        if app.mode == Mode::Normal {
            app.mode = Mode::Peeking;
            return Ok(TransitionResult::Continue);
        } else if app.is_at_bottom() {
            return Ok(TransitionResult::Return(
                value
                    .clone()
                    .follow_cell_path(&app.cell_path.members, false)?,
            ));
        }
    }

    if app.mode == Mode::Peeking {
        if key == &config.keybindings.normal {
            app.mode = Mode::Normal;
            return Ok(TransitionResult::Continue);
        } else if key == &config.keybindings.peeking.all {
            return Ok(TransitionResult::Return(value.clone()));
        } else if key == &config.keybindings.peeking.view {
            app.cell_path.members.pop();
            return Ok(TransitionResult::Return(
                value
                    .clone()
                    .follow_cell_path(&app.cell_path.members, false)?,
            ));
        } else if key == &config.keybindings.peeking.under {
            return Ok(TransitionResult::Return(
                value
                    .clone()
                    .follow_cell_path(&app.cell_path.members, false)?,
            ));
        } else if key == &config.keybindings.peeking.cell_path {
            return Ok(TransitionResult::Return(Value::cell_path(
                app.cell_path.clone(),
                Span::unknown(),
            )));
        }
    }

    if app.mode == Mode::Insert {
        match app.editor.handle_key(key) {
            Some((mode, val)) => {
                app.mode = mode;

                match val {
                    Some(v) => return Ok(TransitionResult::Edit(v)),
                    None => return Ok(TransitionResult::Continue),
                }
            }
            None => return Ok(TransitionResult::Continue),
        }
    }

    Ok(TransitionResult::Continue)
}

#[cfg(test)]
mod tests {
    use console::Key;
    use nu_protocol::{
        ast::{CellPath, PathMember},
        Span, Value,
    };

    use super::{transition_state, App, TransitionResult};
    use crate::{
        app::Mode,
        config::{repr_keycode, Config},
        nu::cell_path::{to_path_member_vec, PM},
    };

    /// {
    ///     l: ["my", "list", "elements"],
    ///     r: {a: 1, b: 2},
    ///     s: "some string",
    ///     i: 123,
    /// }
    fn test_value() -> Value {
        Value::test_record(
            vec!["l", "r", "s", "i"],
            vec![
                Value::test_list(vec![
                    Value::test_string("my"),
                    Value::test_string("list"),
                    Value::test_string("elements"),
                ]),
                Value::test_record(vec!["a", "b"], vec![Value::test_int(1), Value::test_int(2)]),
                Value::test_string("some string"),
                Value::test_int(123),
            ],
        )
    }

    #[test]
    fn switch_modes() {
        let config = Config::default();
        let keybindings = config.clone().keybindings;

        let mut app = App::default();
        let value = test_value();

        assert!(app.mode == Mode::Normal);

        // INSERT -> PEEKING: not allowed
        // PEEKING -> INSERT: not allowed
        let transitions = vec![
            (&keybindings.normal, Mode::Normal),
            // FIXME: non-string editing is not allowed
            // (&keybindings.insert, Mode::Insert),
            (&keybindings.normal, Mode::Normal),
            (&keybindings.peek, Mode::Peeking),
            (&keybindings.normal, Mode::Normal),
        ];

        for (key, expected_mode) in transitions {
            let mode = app.mode.clone();

            let result = transition_state(&key, &config, &mut app, &value).unwrap();

            assert!(
                !result.is_quit(),
                "unexpected exit after pressing {} in {}",
                repr_keycode(key),
                mode,
            );
            assert!(
                app.mode == expected_mode,
                "expected to be in {} after pressing {} in {}, found {}",
                expected_mode,
                repr_keycode(key),
                mode,
                app.mode
            );
        }
    }

    #[test]
    fn quit() {
        let config = Config::default();
        let keybindings = config.clone().keybindings;

        let mut app = App::default();
        let value = test_value();

        let transitions = vec![
            (&keybindings.insert, false),
            (&keybindings.quit, true),
            (&keybindings.normal, false),
            (&keybindings.quit, true),
            (&keybindings.peek, false),
            (&keybindings.quit, true),
        ];

        for (key, exit) in transitions {
            let mode = app.mode.clone();

            let result = transition_state(key, &config, &mut app, &value).unwrap();

            if exit {
                assert!(
                    result.is_quit(),
                    "expected to quit after pressing {} in {} mode",
                    repr_keycode(key),
                    mode
                );
            } else {
                assert!(
                    !result.is_quit(),
                    "expected NOT to quit after pressing {} in {} mode",
                    repr_keycode(key),
                    mode
                );
            }
        }
    }

    fn repr_path_member_vec(members: &[PathMember]) -> String {
        format!(
            "$.{}",
            members
                .iter()
                .map(|m| {
                    match m {
                        PathMember::Int { val, .. } => val.to_string(),
                        PathMember::String { val, .. } => val.to_string(),
                    }
                })
                .collect::<Vec<String>>()
                .join(".")
        )
    }

    #[test]
    fn navigate_the_data() {
        let config = Config::default();
        let nav = config.clone().keybindings.navigation;

        let value = test_value();
        let mut app = App::from_value(&value);

        assert!(!app.is_at_bottom());
        assert_eq!(app.cell_path.members, to_path_member_vec(vec![PM::S("l")]));

        let transitions = vec![
            (&nav.up, vec![PM::S("i")], false),
            (&nav.up, vec![PM::S("s")], false),
            (&nav.up, vec![PM::S("r")], false),
            (&nav.up, vec![PM::S("l")], false),
            (&nav.down, vec![PM::S("r")], false),
            (&nav.left, vec![PM::S("r")], false),
            (&nav.right, vec![PM::S("r"), PM::S("a")], false),
            (&nav.right, vec![PM::S("r"), PM::S("a")], true),
            (&nav.up, vec![PM::S("r"), PM::S("a")], true),
            (&nav.down, vec![PM::S("r"), PM::S("a")], true),
            (&nav.left, vec![PM::S("r"), PM::S("a")], false),
            (&nav.down, vec![PM::S("r"), PM::S("b")], false),
            (&nav.right, vec![PM::S("r"), PM::S("b")], true),
            (&nav.up, vec![PM::S("r"), PM::S("b")], true),
            (&nav.down, vec![PM::S("r"), PM::S("b")], true),
            (&nav.left, vec![PM::S("r"), PM::S("b")], false),
            (&nav.up, vec![PM::S("r"), PM::S("a")], false),
            (&nav.up, vec![PM::S("r"), PM::S("b")], false),
            (&nav.left, vec![PM::S("r")], false),
            (&nav.down, vec![PM::S("s")], false),
            (&nav.left, vec![PM::S("s")], false),
            (&nav.right, vec![PM::S("s")], true),
            (&nav.up, vec![PM::S("s")], true),
            (&nav.down, vec![PM::S("s")], true),
            (&nav.left, vec![PM::S("s")], false),
            (&nav.down, vec![PM::S("i")], false),
            (&nav.left, vec![PM::S("i")], false),
            (&nav.right, vec![PM::S("i")], true),
            (&nav.up, vec![PM::S("i")], true),
            (&nav.down, vec![PM::S("i")], true),
            (&nav.left, vec![PM::S("i")], false),
            (&nav.down, vec![PM::S("l")], false),
            (&nav.left, vec![PM::S("l")], false),
            (&nav.right, vec![PM::S("l"), PM::I(0)], false),
            (&nav.right, vec![PM::S("l"), PM::I(0)], true),
            (&nav.up, vec![PM::S("l"), PM::I(0)], true),
            (&nav.down, vec![PM::S("l"), PM::I(0)], true),
            (&nav.left, vec![PM::S("l"), PM::I(0)], false),
            (&nav.down, vec![PM::S("l"), PM::I(1)], false),
            (&nav.right, vec![PM::S("l"), PM::I(1)], true),
            (&nav.up, vec![PM::S("l"), PM::I(1)], true),
            (&nav.down, vec![PM::S("l"), PM::I(1)], true),
            (&nav.left, vec![PM::S("l"), PM::I(1)], false),
            (&nav.down, vec![PM::S("l"), PM::I(2)], false),
            (&nav.right, vec![PM::S("l"), PM::I(2)], true),
            (&nav.up, vec![PM::S("l"), PM::I(2)], true),
            (&nav.down, vec![PM::S("l"), PM::I(2)], true),
            (&nav.left, vec![PM::S("l"), PM::I(2)], false),
            (&nav.up, vec![PM::S("l"), PM::I(1)], false),
            (&nav.up, vec![PM::S("l"), PM::I(0)], false),
            (&nav.up, vec![PM::S("l"), PM::I(2)], false),
            (&nav.left, vec![PM::S("l")], false),
        ];

        for (key, cell_path, bottom) in transitions {
            let expected = to_path_member_vec(cell_path);
            transition_state(key, &config, &mut app, &value).unwrap();

            if bottom {
                assert!(
                    app.is_at_bottom(),
                    "expected to be at the bottom after pressing {}",
                    repr_keycode(key)
                );
            } else {
                assert!(
                    !app.is_at_bottom(),
                    "expected NOT to be at the bottom after pressing {}",
                    repr_keycode(key)
                );
            }
            assert_eq!(
                app.cell_path.members,
                expected,
                "expected to be at {:?}, found {:?}",
                repr_path_member_vec(&expected),
                repr_path_member_vec(&app.cell_path.members)
            );
        }
    }

    fn run_peeking_scenario(
        transitions: Vec<(&Key, bool, Option<Value>)>,
        config: &Config,
        value: &Value,
    ) {
        let mut app = App::from_value(&value);

        for (key, exit, expected) in transitions {
            let mode = app.mode.clone();

            let result = transition_state(key, &config, &mut app, &value).unwrap();

            if exit {
                assert!(
                    result.is_quit(),
                    "expected to peek some data after pressing {} in {} mode",
                    repr_keycode(key),
                    mode
                );
            } else {
                assert!(
                    !result.is_quit(),
                    "expected NOT to peek some data after pressing {} in {} mode",
                    repr_keycode(key),
                    mode
                );
            }

            match expected {
                Some(value) => match result {
                    TransitionResult::Return(val) => {
                        assert_eq!(
                            value,
                            val,
                            "unexpected data after pressing {} in {} mode",
                            repr_keycode(key),
                            mode
                        )
                    }
                    _ => panic!(
                        "did expect output data after pressing {} in {} mode",
                        repr_keycode(key),
                        mode
                    ),
                },
                None => match result {
                    TransitionResult::Return(_) => panic!(
                        "did NOT expect output data after pressing {} in {} mode",
                        repr_keycode(key),
                        mode
                    ),
                    _ => {}
                },
            }
        }
    }

    #[test]
    fn peek_data() {
        let config = Config::default();
        let keybindings = config.clone().keybindings;

        let value = test_value();

        let peek_all_from_top = vec![
            (&keybindings.peek, false, None),
            (&keybindings.peeking.all, true, Some(value.clone())),
        ];
        run_peeking_scenario(peek_all_from_top, &config, &value);

        let peek_current_from_top = vec![
            (&keybindings.peek, false, None),
            (&keybindings.peeking.view, true, Some(value.clone())),
        ];
        run_peeking_scenario(peek_current_from_top, &config, &value);

        let go_in_the_data_and_peek_all_and_current = vec![
            (&keybindings.navigation.down, false, None),
            (&keybindings.navigation.right, false, None), // on {r: {a: 1, b: 2}}
            (&keybindings.peek, false, None),
            (&keybindings.peeking.all, true, Some(value.clone())),
            (
                &keybindings.peeking.view,
                true,
                Some(Value::test_record(
                    vec!["a", "b"],
                    vec![Value::test_int(1), Value::test_int(2)],
                )),
            ),
        ];
        run_peeking_scenario(go_in_the_data_and_peek_all_and_current, &config, &value);

        let go_in_the_data_and_peek_under = vec![
            (&keybindings.navigation.down, false, None),
            (&keybindings.navigation.right, false, None), // on {r: {a: 1, b: 2}}
            (&keybindings.peek, false, None),
            (&keybindings.peeking.all, true, Some(value.clone())),
            (&keybindings.peeking.under, true, Some(Value::test_int(1))),
        ];
        run_peeking_scenario(go_in_the_data_and_peek_under, &config, &value);

        let go_in_the_data_and_peek_cell_path = vec![
            (&keybindings.navigation.down, false, None), // on {r: {a: 1, b: 2}}
            (&keybindings.navigation.right, false, None), // on {a: 1}
            (&keybindings.peek, false, None),
            (
                &keybindings.peeking.cell_path,
                true,
                Some(Value::test_cell_path(CellPath {
                    members: vec![
                        PathMember::String {
                            val: "r".into(),
                            span: Span::test_data(),
                            optional: false,
                        },
                        PathMember::String {
                            val: "a".into(),
                            span: Span::test_data(),
                            optional: false,
                        },
                    ],
                })),
            ),
        ];
        run_peeking_scenario(go_in_the_data_and_peek_cell_path, &config, &value);

        let peek_at_the_bottom = vec![
            (&keybindings.navigation.right, false, None), // on l: ["my", "list", "elements"],
            (&keybindings.navigation.right, false, None), // on "my"
            (&keybindings.peek, true, Some(Value::test_string("my"))),
        ];
        run_peeking_scenario(peek_at_the_bottom, &config, &value);
    }
}
