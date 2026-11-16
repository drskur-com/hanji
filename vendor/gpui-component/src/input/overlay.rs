use std::collections::HashMap;
use std::rc::Rc;

use gpui::{AnyElement, App, Entity, EntityId, Global, IntoElement, WeakEntity, Window};
use ropey::Rope;

use super::{
    InputBaseState, InputModeKind,
    popovers::{CodeActionMenu, CompletionMenu, DiagnosticPopover, HoverPopover},
    search::SearchPanel,
};
use gpui_base::input::CompletionHighlighter;

struct InputOverlayRegistry<M: OverlayMode> {
    hosts: HashMap<EntityId, (WeakEntity<InputBaseState<M>>, InputOverlayHost<M>)>,
}

impl<M: OverlayMode> Default for InputOverlayRegistry<M> {
    fn default() -> Self {
        Self {
            hosts: HashMap::default(),
        }
    }
}

impl<M: OverlayMode> Global for InputOverlayRegistry<M> {}

struct InputOverlayHost<M: OverlayMode> {
    search: Entity<SearchPanel<M>>,
    search_signature: SearchSignature,
    /// The language-feature popovers. Only a code editor has them.
    lsp: Option<LspOverlays>,
}

/// Identifies one version of the search session, as the search panel shows it.
///
/// Everything the panel renders from is here, so an unchanged frame compares
/// equal and rebuilds nothing. `activation_revision` is what separates "the
/// session is open" from "the user just asked for search again": re-invoking an
/// open session leaves every other field alone, and without the revision the
/// second request would compare equal and be dropped.
#[derive(PartialEq, Eq, Default)]
struct SearchSignature {
    open: bool,
    replace_mode: bool,
    query: String,
    anchor_offset: Option<usize>,
    activation_revision: u64,
}

/// Identifies one version of an overlay's content.
///
/// Sync runs on every frame, so the check for "did this change" must not touch
/// the content itself: the completion list can hold hundreds of items, each
/// with its own documentation. The engine bumps a revision when it swaps the
/// content, and the popovers are keyed by cheap identity instead — a revision,
/// an `Rc` pointer, or a range.
#[derive(PartialEq, Eq, Default)]
struct OverlaySignature {
    open: bool,
    revision: u64,
}

/// The popovers driven by language features: completion, code actions, hover
/// and diagnostics. They belong to a code editor, so they are built and synced
/// by [`OverlayMode`], where the state's kind is concrete.
pub(crate) struct LspOverlays {
    completion: Entity<CompletionMenu>,
    code_actions: Entity<CodeActionMenu>,
    hover: Option<Entity<HoverPopover>>,
    diagnostic: Option<Entity<DiagnosticPopover>>,
    completion_signature: OverlaySignature,
    code_action_signature: OverlaySignature,
    hover_signature: Option<std::ops::Range<usize>>,
    diagnostic_signature: Option<std::rc::Rc<gpui_base::input::DiagnosticEntry>>,
    /// Highlighter last copied onto the completion menu.
    ///
    /// Compared by `Rc` identity, so swapping the host's closure repaints an
    /// open menu without rebuilding its items.
    completion_highlight: Option<Rc<CompletionHighlighter>>,
}

/// What the state read out of the engine for one sync pass.
///
/// Deliberately free of content: only what is needed to decide whether a
/// popover is shown and whether it changed. The content is read from the state
/// again, on the frames where it actually did.
pub(crate) struct LspSnapshot {
    completion: OverlaySignature,
    completion_start: Option<usize>,
    code_action: OverlaySignature,
    hover: Option<std::ops::Range<usize>>,
    diagnostic: Option<std::rc::Rc<gpui_base::input::DiagnosticEntry>>,
    cursor: usize,
}

impl LspSnapshot {
    fn has_overlay(&self) -> bool {
        self.completion.open
            || self.code_action.open
            || self.hover.is_some()
            || self.diagnostic.is_some()
    }
}

/// What the UI layer needs from each kind of input.
///
/// The UI layer is generic over the mode, so it cannot name the editor's state
/// type. This trait is the seam: the code-editor implementation is the only one
/// that builds language-feature popovers, and inside it the state is an
/// `Entity<EditorState>`, which those popovers take.
///
/// Entirely internal: nothing public is generic over the mode any more.
pub(crate) trait OverlayMode: InputModeKind + Sized {
    /// Reads the language-feature state this mode shows, if any.
    fn lsp_snapshot(_state: &InputBaseState<Self>, _cx: &App) -> Option<LspSnapshot> {
        None
    }

    /// Installs the routing that lets an open menu consume actions first.
    fn install_action_handler(_state: &Entity<InputBaseState<Self>>, _cx: &mut App) {}

    fn build_lsp(
        _state: &Entity<InputBaseState<Self>>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<LspOverlays> {
        None
    }

    fn sync_lsp(
        _lsp: &mut LspOverlays,
        _state: &Entity<InputBaseState<Self>>,
        _snapshot: &LspSnapshot,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}

impl OverlayMode for crate::input::InputMode {}
impl OverlayMode for crate::input::TextareaMode {}

impl OverlayMode for crate::input::EditorMode {
    fn install_action_handler(state: &Entity<InputBaseState<Self>>, cx: &mut App) {
        let id = state.entity_id();
        state.update(cx, move |state, _| {
            state.set_overlay_action_handler(move |kind, action, window, cx| {
                let menus = cx
                    .try_global::<InputOverlayRegistry<Self>>()
                    .and_then(|registry| registry.hosts.get(&id))
                    .and_then(|(_, host)| host.lsp.as_ref())
                    .map(|lsp| (lsp.completion.clone(), lsp.code_actions.clone()));
                let Some((completion, code_actions)) = menus else {
                    return false;
                };
                match kind {
                    gpui_base::input::InputOverlayKind::Completion => {
                        completion.update(cx, |menu, cx| menu.handle_action(action, window, cx))
                    }
                    gpui_base::input::InputOverlayKind::CodeAction => {
                        code_actions.update(cx, |menu, cx| menu.handle_action(action, window, cx))
                    }
                }
            });
        });
    }

    fn lsp_snapshot(state: &InputBaseState<Self>, _cx: &App) -> Option<LspSnapshot> {
        let completion = state.completion_menu_state();
        let code_actions = state.code_action_menu_state();
        Some(LspSnapshot {
            completion: OverlaySignature {
                open: completion.open,
                revision: completion.revision(),
            },
            completion_start: completion.trigger_start_offset,
            code_action: OverlaySignature {
                open: code_actions.open,
                revision: code_actions.revision(),
            },
            hover: state
                .hover_popover()
                .map(|popover| popover.symbol_range.clone()),
            diagnostic: state.diagnostic_popover(),
            cursor: state.cursor(),
        })
    }

    fn build_lsp(
        state: &Entity<InputBaseState<Self>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<LspOverlays> {
        Some(LspOverlays {
            completion: CompletionMenu::new(state.clone(), window, cx),
            code_actions: CodeActionMenu::new(state.clone(), window, cx),
            hover: None,
            diagnostic: None,
            completion_signature: OverlaySignature::default(),
            code_action_signature: OverlaySignature::default(),
            hover_signature: None,
            diagnostic_signature: None,
            completion_highlight: None,
        })
    }

    fn sync_lsp(
        lsp: &mut LspOverlays,
        state: &Entity<InputBaseState<Self>>,
        snapshot: &LspSnapshot,
        window: &mut Window,
        cx: &mut App,
    ) {
        let cursor = snapshot.cursor;

        let highlight = state.read(cx).lsp().completion_highlighter().clone();
        if lsp
            .completion_highlight
            .as_ref()
            .is_none_or(|current| !Rc::ptr_eq(current, &highlight))
        {
            lsp.completion_highlight = Some(highlight.clone());
            lsp.completion
                .update(cx, |menu, cx| menu.set_highlight(highlight, cx));
        }

        if snapshot.completion != lsp.completion_signature {
            lsp.completion_signature = OverlaySignature {
                open: snapshot.completion.open,
                revision: snapshot.completion.revision,
            };
            let open = snapshot.completion.open;
            let start = snapshot.completion_start;
            // Read the items only now, on a frame where they changed.
            let (query, items) = {
                let menu = state.read(cx).completion_menu_state();
                (menu.query.clone(), menu.items.clone())
            };
            lsp.completion.update(cx, |menu, cx| {
                if open {
                    menu.update_query(start.unwrap_or(cursor), query);
                    menu.show(cursor, items, window, cx);
                } else {
                    menu.hide(cx);
                }
            });
        }

        if snapshot.code_action != lsp.code_action_signature {
            lsp.code_action_signature = OverlaySignature {
                open: snapshot.code_action.open,
                revision: snapshot.code_action.revision,
            };
            let open = snapshot.code_action.open;
            let items = state.read(cx).code_action_menu_state().items.clone();
            lsp.code_actions.update(cx, |menu, cx| {
                if open {
                    menu.show(cursor, items, window, cx);
                } else {
                    menu.hide(cx);
                }
            });
        }

        // A hover popover is anchored to one symbol, so a new range means new
        // content and the same range means the same content.
        if snapshot.hover != lsp.hover_signature {
            lsp.hover_signature = snapshot.hover.clone();
            let popover = state
                .read(cx)
                .hover_popover()
                .map(|popover| (popover.symbol_range.clone(), popover.hover.clone()));
            lsp.hover = popover.map(|(symbol_range, hover)| {
                HoverPopover::new(state.clone(), symbol_range, &hover, cx)
            });
        }

        // The engine hands out the diagnostic behind an `Rc`, so pointer
        // identity already answers "is this the same entry".
        let diagnostic_changed = match (&snapshot.diagnostic, &lsp.diagnostic_signature) {
            (Some(new), Some(old)) => !std::rc::Rc::ptr_eq(new, old),
            (None, None) => false,
            _ => true,
        };
        if diagnostic_changed {
            lsp.diagnostic_signature = snapshot.diagnostic.clone();
            lsp.diagnostic = snapshot
                .diagnostic
                .as_deref()
                .map(|entry| DiagnosticPopover::new(entry, state.clone(), cx));
        }
    }
}

#[derive(Default)]
pub(super) struct InputOverlays {
    pub search: Option<AnyElement>,
    pub floating: Vec<AnyElement>,
}

impl InputOverlays {
    #[cfg(test)]
    fn len(&self) -> usize {
        usize::from(self.search.is_some()) + self.floating.len()
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.search.is_none() && self.floating.is_empty()
    }
}

impl<M: OverlayMode> InputOverlayHost<M> {
    fn new(state: Entity<InputBaseState<M>>, window: &mut Window, cx: &mut App) -> Self {
        Self {
            search: SearchPanel::new(state.clone(), window, cx),
            search_signature: SearchSignature::default(),
            lsp: M::build_lsp(&state, window, cx),
        }
    }

    fn sync(
        &mut self,
        state: &Entity<InputBaseState<M>>,
        window: &mut Window,
        cx: &mut App,
    ) -> InputOverlays {
        let snapshot = M::lsp_snapshot(state.read(cx), cx);
        let (search_open, replace_mode, activation_revision, search_session) = {
            let state = state.read(cx);
            let search = state.search_session();
            (
                search.open,
                search.replace_mode,
                state.search_activation_revision(),
                search.clone(),
            )
        };

        let panel_replace_mode = self.search.read(cx).replace_mode();
        self.search
            .update(cx, |panel, _| panel.sync_session(&search_session));

        let search_signature = SearchSignature {
            open: search_open,
            replace_mode,
            query: search_session.query.clone(),
            anchor_offset: search_session.anchor_offset,
            activation_revision,
        };
        if search_signature != self.search_signature {
            // The panel writes query and replacement-mode changes back into the
            // session. Re-showing it for that echo would select the query or steal
            // focus from replacement, so preserve changes the panel already holds.
            // External mode changes still move focus out of a hidden replace field.
            //
            // The anchor offset must not gate the echo: every query change
            // rewrites it, including the ones the panel itself typed, so
            // comparing it reads the panel's own input back as an external
            // change. The re-show then selects the field mid-typing, and a
            // selection-swapping preedit (CJK IME) loses the text it had
            // committed. The revision still catches an explicit re-invoke.
            //
            // The field's value cannot gate it either. A committed syllable
            // and the next composition's preedit can both land before the
            // next frame: the session holds the commit ("하") while the field
            // already shows the preedit ("하ㄴ"), so comparing them would read
            // the panel's own query as external and select the field exactly
            // when the second syllable starts. What the panel last wrote is
            // the stable mark of its own echo.
            let was = &self.search_signature;
            let query_echo = search_open
                && was.open
                && (was.replace_mode == search_signature.replace_mode
                    || panel_replace_mode == search_signature.replace_mode)
                && was.activation_revision == search_signature.activation_revision
                && self.search.read(cx).wrote_query(&search_session.query);
            self.search_signature = search_signature;
            if !query_echo {
                self.search.update(cx, |panel, cx| {
                    if search_open {
                        let selected = Rope::from(search_session.query.clone());
                        let visible = search_session.anchor_offset.map(|offset| offset..offset);
                        panel.show_with_focus(
                            &selected,
                            replace_mode,
                            visible,
                            !cfg!(test),
                            window,
                            cx,
                        );
                    } else {
                        panel.hide_with_focus(!cfg!(test), window, cx);
                    }
                });
            }
        }

        if let (Some(lsp), Some(snapshot)) = (self.lsp.as_mut(), snapshot.as_ref()) {
            M::sync_lsp(lsp, state, snapshot, window, cx);
        }

        let search = search_open.then(|| self.search.clone().into_any_element());
        let mut floating = Vec::with_capacity(4);
        if let (Some(lsp), Some(snapshot)) = (self.lsp.as_ref(), snapshot.as_ref()) {
            if snapshot.completion.open {
                floating.push(lsp.completion.clone().into_any_element());
            }
            if snapshot.code_action.open {
                floating.push(lsp.code_actions.clone().into_any_element());
            }
            if let Some(hover) = lsp.hover.as_ref() {
                floating.push(hover.clone().into_any_element());
            }
            if let Some(diagnostic) = lsp.diagnostic.as_ref() {
                floating.push(diagnostic.clone().into_any_element());
            }
        }
        InputOverlays { search, floating }
    }
}

pub(super) fn render_overlays<M: OverlayMode>(
    state: &Entity<InputBaseState<M>>,
    window: &mut Window,
    cx: &mut App,
) -> InputOverlays {
    M::install_action_handler(state, cx);
    let has_overlay = {
        let state = state.read(cx);
        state.search_session().open
            || M::lsp_snapshot(state, cx).is_some_and(|lsp| lsp.has_overlay())
    };
    if !has_overlay {
        if cx.has_global::<InputOverlayRegistry<M>>() {
            let registry = cx.global_mut::<InputOverlayRegistry<M>>();
            registry.hosts.remove(&state.entity_id());
            registry
                .hosts
                .retain(|_, (owner, _)| owner.upgrade().is_some());
        }
        return InputOverlays::default();
    }

    if !cx.has_global::<InputOverlayRegistry<M>>() {
        cx.set_global(InputOverlayRegistry::<M>::default());
    }

    let id = state.entity_id();
    let mut host = cx
        .global_mut::<InputOverlayRegistry<M>>()
        .hosts
        .remove(&id)
        .map(|(_, host)| host)
        .unwrap_or_else(|| InputOverlayHost::new(state.clone(), window, cx));
    let overlays = host.sync(state, window, cx);
    cx.global_mut::<InputOverlayRegistry<M>>()
        .hosts
        .insert(id, (state.downgrade(), host));
    overlays
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, ParentElement, Render, SharedString, Styled, div};
    use gpui_base::input::CodeActionItem;
    use gpui_base::input::DiagnosticEntry;
    use lsp_types::{CodeAction, CompletionItem, Hover, HoverContents, MarkedString};

    struct OverlayProbe {
        state: Entity<crate::input::EditorState>,
    }

    impl Render for OverlayProbe {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
        }
    }

    /// A probe that paints the editor itself, so the state records a layout
    /// (`last_layout`) and a query change anchors to a real viewport offset.
    struct PaintedEditorProbe {
        state: Entity<crate::input::EditorState>,
    }

    impl Render for PaintedEditorProbe {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            use gpui::{ParentElement as _, Styled as _};
            div().max_size_full().child(crate::input::Editor::new(&self.state))
        }
    }

    /// Asking for search again, while it is already open, must re-select the query.
    ///
    /// Re-invoking search leaves every other part of the session identical, so a
    /// sync keyed only on that state reads it as an unchanged frame and drops the
    /// request: the field keeps whatever caret the user left in it and the
    /// shortcut looks dead. `activation_revision` is what carries the second
    /// request through, and the echo guard below it is what must survive that.
    #[gpui::test]
    fn reopening_search_reselects_the_query(cx: &mut gpui::TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|window, cx| OverlayProbe {
            state: cx.new(|cx| crate::input::EditorState::new(window, cx).searchable(true)),
        });
        let state = probe.read_with(cx, |probe, _| probe.state.clone());

        cx.update(|window, cx| {
            state.update(cx, |state, cx| {
                state.set_value("foo bar foo", window, cx);
                state.set_selected_range(0..3, cx);
                state.open_search(false, cx);
            });

            let mut host = InputOverlayHost::new(state.clone(), window, cx);
            host.sync(&state, window, cx);

            let search_input = host.search.read(cx).search_input().clone();
            assert_eq!(search_input.read(cx).value(), "foo");
            assert_eq!(
                search_input.read(cx).selected_range(),
                0..3,
                "opening search selects the query it seeded the field with"
            );

            // The user drops the caret into the field. An untouched frame must
            // leave it there — this is the echo guard, and a fix that reshows the
            // panel on every frame would fail here instead of silently annoying.
            search_input.update(cx, |input, cx| input.unselect(window, cx));
            host.sync(&state, window, cx);
            assert!(
                search_input.read(cx).selected_range().is_empty(),
                "an unchanged frame must not touch the field"
            );

            // The shortcut again, with the session already open and nothing else
            // about it moved.
            state.update(cx, |state, cx| state.open_search(false, cx));
            host.sync(&state, window, cx);
            assert_eq!(
                search_input.read(cx).selected_range(),
                0..3,
                "re-invoking search must select the query again"
            );
        });
    }

    /// The search field of the host a painted editor registered.
    ///
    /// Painting an editor whose search is open builds the one host production
    /// uses, so a test that types has to reach that panel. A second host on the
    /// same state answers the session from a field of its own, and the two then
    /// disagree about what the query is.
    fn painted_search_input<C: gpui::AppContext>(
        state: &Entity<crate::input::EditorState>,
        cx: &C,
    ) -> Entity<crate::input::InputState> {
        let id = state.entity_id();
        cx.read_global::<InputOverlayRegistry<crate::input::EditorMode>, _>(|registry, cx| {
            let (_, host) = registry
                .hosts
                .get(&id)
                .expect("painting an open search registers a host");
            host.search
                .read_with(cx, |panel, _| panel.search_input().clone())
        })
    }

    /// Typing in the search field must not re-show the panel.
    ///
    /// Every query change rewrites the session's anchor offset, including the
    /// ones the panel itself typed. Reading that rewrite as an external change
    /// re-invoked `show_with_focus` on the next frame, which selected the whole
    /// field mid-typing: the next IME preedit (CJK input) then replaced the
    /// selection and the text the user had committed disappeared.
    #[gpui::test]
    fn typing_in_the_search_field_keeps_the_field_untouched(cx: &mut gpui::TestAppContext) {
        use gpui::EntityInputHandler as _;

        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|window, cx| PaintedEditorProbe {
            state: cx.new(|cx| crate::input::EditorState::new(window, cx).searchable(true)),
        });
        let state = probe.read_with(cx, |probe, _| probe.state.clone());

        cx.update(|window, cx| {
            state.update(cx, |state, cx| {
                state.set_value("foo bar foo", window, cx);
                state.open_search(false, cx);
            });
        });
        // Painting records a layout, so a query change anchors to a real
        // viewport offset, and registers the host every later frame syncs.
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let search_input = painted_search_input(&state, cx);

        // The caret sits in the field, as it does after typing.
        cx.update(|window, cx| {
            search_input.update(cx, |input, cx| input.unselect(window, cx));
        });

        // The user types a character: the field holds it and the panel's change
        // subscription writes it into the session, which re-anchors the query to
        // the viewport. The frame that follows must leave the field alone.
        cx.update(|window, cx| {
            search_input.update(cx, |input, cx| {
                input.replace_text_in_range(None, "f", window, cx);
            });
        });
        cx.update(|window, cx| {
            let session = state.read(cx).search_session();
            assert_eq!(session.query, "f");
            assert!(
                session.anchor_offset.is_some(),
                "the painted layout gives a typed query an anchor, and only a \
                 second write of the same query — the re-show — would clear it"
            );
            window.draw(cx).clear(cx);
            assert!(
                search_input.read(cx).selected_range().is_empty(),
                "the panel's own query change must not re-select the field"
            );
            assert_eq!(search_input.read(cx).value(), "f");
        });

        // The user navigates (Enter) and keeps typing: the editor scrolls, so
        // the next query change anchors to a different offset. That still must
        // not read as an external change and re-select the field mid-typing.
        cx.update(|window, cx| {
            state.update(cx, |state, cx| {
                state.next_search_match(cx);
            });
            search_input.update(cx, |input, cx| {
                input.replace_text_in_range(None, "o", window, cx);
            });
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert!(
                search_input.read(cx).selected_range().is_empty(),
                "a query change after navigation must not re-select the field"
            );
            assert_eq!(search_input.read(cx).value(), "fo");
        });
    }

    /// A syllable committed while the next composition is already marked is
    /// still the panel's own query change.
    ///
    /// The Korean IME commits the finished syllable in one input update and
    /// marks the next one in the one after it, so the session holds the commit
    /// ("하") while the field already shows the preedit ("하ㄴ") by the time a
    /// frame syncs the panel. Reading the field's value as the query then
    /// looked like a change from outside, which re-showed the panel and
    /// selected the field; the next key replaced that selection and the
    /// committed syllable vanished.
    ///
    /// The host is driven by hand here. Painting the editor would sync it at
    /// the end of every update, and the frame that has to catch the divergence
    /// is exactly the one that must be deferred past both IME calls.
    #[gpui::test]
    fn a_preedit_after_a_commit_is_not_an_external_query_change(cx: &mut gpui::TestAppContext) {
        use gpui::EntityInputHandler as _;

        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|window, cx| OverlayProbe {
            state: cx.new(|cx| crate::input::EditorState::new(window, cx).searchable(true)),
        });
        let state = probe.read_with(cx, |probe, _| probe.state.clone());

        cx.update(|window, cx| {
            state.update(cx, |state, cx| {
                state.set_value("하나 둘", window, cx);
                state.open_search(false, cx);
            });
        });
        let mut host = cx.update(|window, cx| InputOverlayHost::new(state.clone(), window, cx));
        // The frame that opens the panel.
        cx.update(|window, cx| {
            host.sync(&state, window, cx);
        });
        let search_input = host.search.read_with(cx, |panel, _| panel.search_input().clone());

        // The IME composes the first syllable and commits it: the panel's
        // change subscription writes the commit into the session.
        cx.update(|window, cx| {
            search_input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "ㅎ", Some(1..1), window, cx);
                input.replace_and_mark_text_in_range(None, "하", Some(1..1), window, cx);
                input.replace_text_in_range(None, "하", window, cx);
            });
        });
        // The next key starts the following syllable. Marked text emits no
        // change, so the session keeps the commit while the field shows both.
        cx.update(|window, cx| {
            search_input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "ㄴ", Some(1..1), window, cx);
            });
        });
        cx.update(|_, cx| {
            assert_eq!(
                state.read(cx).search_session().query,
                "하",
                "the commit must have reached the session, or this test proves nothing"
            );
            assert_eq!(search_input.read(cx).value(), "하ㄴ");
        });

        // The frame that syncs the panel.
        cx.update(|window, cx| {
            host.sync(&state, window, cx);
        });
        cx.update(|_, cx| {
            assert_eq!(
                search_input.read(cx).value(),
                "하ㄴ",
                "the next composition's preedit must survive the frame"
            );
            assert!(
                search_input.read(cx).selected_range().is_empty(),
                "the commit must not re-select the field once the preedit starts"
            );
        });
    }
    /// Reopening search must preserve navigation already owned by Base.
    ///
    /// Closing and reopening Find keeps the previous occurrence in browsers.
    /// Neither Base reopening the session nor the styled panel echoing its
    /// retained query may reset the current match to zero.
    #[gpui::test]
    fn reopening_search_panel_preserves_the_previous_match(cx: &mut gpui::TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|window, cx| OverlayProbe {
            state: cx.new(|cx| crate::input::EditorState::new(window, cx).searchable(true)),
        });
        let state = probe.read_with(cx, |probe, _| probe.state.clone());

        cx.update(|window, cx| {
            state.update(cx, |state, cx| {
                state.set_value("foo bar foo baz foo", window, cx);
                state.open_search(false, cx);
                state.set_search_query("foo", true, cx);
                assert_eq!(state.next_search_match(cx), Some(8..11));
                assert_eq!(state.search_session().matcher.current_match_index(), 1);
                state.close_search(cx);
                state.open_search(false, cx);
                assert_eq!(state.search_session().matcher.current_match_index(), 1);
            });

            let mut host = InputOverlayHost::new(state.clone(), window, cx);
            host.sync(&state, window, cx);

            state.read_with(cx, |state, _| {
                assert_eq!(state.search_session().matcher.current_match_index(), 1);
                assert_eq!(state.search_session().matcher.label(), "2/3");
            });

            state.update(cx, |state, cx| {
                assert_eq!(state.next_search_match(cx), Some(16..19));
            });
        });
    }

    /// A frame that changed nothing must not rebuild the popovers.
    ///
    /// Sync runs every frame, so the change check has to be cheap and stable.
    /// It is keyed on a revision the engine bumps when it swaps the content;
    /// forgetting to bump, or comparing the content itself again, both show up
    /// here.
    #[gpui::test]
    fn unchanged_content_keeps_the_overlay_signature(cx: &mut gpui::TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|window, cx| OverlayProbe {
            state: cx.new(|cx| crate::input::EditorState::new(window, cx).language("sql")),
        });
        let state = probe.read_with(cx, |probe, _| probe.state.clone());

        cx.update(|window, cx| {
            state.update(cx, |state, cx| {
                state.present_completion_items(
                    0,
                    "f",
                    vec![CompletionItem {
                        label: "foo".into(),
                        ..Default::default()
                    }],
                    cx,
                );
            });

            let mut host = InputOverlayHost::new(state.clone(), window, cx);
            host.sync(&state, window, cx);
            let after_first = host.lsp.as_ref().unwrap().completion_signature.revision;

            // A second pass over untouched content.
            host.sync(&state, window, cx);
            assert_eq!(
                host.lsp.as_ref().unwrap().completion_signature.revision,
                after_first,
                "an unchanged frame must not bump the signature"
            );

            // Swapping the items must be noticed.
            state.update(cx, |state, cx| {
                state.present_completion_items(
                    0,
                    "f",
                    vec![CompletionItem {
                        label: "bar".into(),
                        ..Default::default()
                    }],
                    cx,
                );
            });
            host.sync(&state, window, cx);
            assert_ne!(
                host.lsp.as_ref().unwrap().completion_signature.revision,
                after_first,
                "new items must be noticed"
            );
        });
    }

    #[gpui::test]
    fn facade_materializes_all_base_overlay_sessions(cx: &mut gpui::TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|window, cx| OverlayProbe {
            state: cx.new(|cx| {
                crate::input::EditorState::new(window, cx)
                    .language("sql")
                    .searchable(true)
                    .replaceable(true)
            }),
        });
        let state = probe.read_with(cx, |probe, _| probe.state.clone());
        cx.update(|window, cx| {
            assert!(render_overlays(&state, window, cx).is_empty());
            assert!(state.read(cx).has_overlay_action_handler());
            state.update(cx, |state, cx| {
                state.set_value("foo bar foo", window, cx);
                state.set_selected_range(4..7, cx);
                state.open_search(true, cx);
                assert_eq!(state.search_session().query, "bar");
                assert!(state.search_session().replace_mode);
                state.present_completion_items(
                    0,
                    "f",
                    vec![CompletionItem {
                        label: "foo".into(),
                        ..Default::default()
                    }],
                    cx,
                );
                state.present_code_actions(
                    vec![CodeActionItem {
                        provider_id: SharedString::from("test"),
                        action: CodeAction {
                            title: "Fix".into(),
                            ..Default::default()
                        },
                    }],
                    cx,
                );
                state.present_hover(
                    0..1,
                    Hover {
                        contents: HoverContents::Scalar(MarkedString::String("docs".into())),
                        range: None,
                    },
                    cx,
                );
                state.present_diagnostic(DiagnosticEntry::default(), cx);
            });

            let mut host = InputOverlayHost::new(state.clone(), window, cx);
            let overlays = host.sync(&state, window, cx);
            assert!(overlays.search.is_some());
            assert_eq!(overlays.floating.len(), 4);
            assert_eq!(overlays.len(), 5);
            assert_eq!(render_overlays(&state, window, cx).len(), 5);
            assert!(
                cx.global::<InputOverlayRegistry<crate::input::EditorMode>>()
                    .hosts
                    .contains_key(&state.entity_id())
            );

            state.update(cx, |state, cx| {
                assert!(state.route_overlay_action(Box::new(super::super::Escape), window, cx));
                assert!(!state.completion_menu_state().open);
                state.dismiss_code_action_overlay(cx);
                state.close_search(cx);
                state.clear_hover_state(cx);
                state.clear_diagnostic_popover(cx);
            });
            assert!(host.sync(&state, window, cx).is_empty());
            assert!(render_overlays(&state, window, cx).is_empty());
            assert!(
                !cx.global::<InputOverlayRegistry<crate::input::EditorMode>>()
                    .hosts
                    .contains_key(&state.entity_id())
            );
        });

        let dropped_owner = cx.update(|window, cx| {
            let ephemeral = cx.new(|cx| {
                crate::input::EditorState::new(window, cx)
                    .language("sql")
                    .searchable(true)
            });
            ephemeral.update(cx, |state, cx| state.open_search(false, cx));
            assert_eq!(render_overlays(&ephemeral, window, cx).len(), 1);
            ephemeral.update(cx, |state, cx| state.close_search(cx));
            assert!(render_overlays(&ephemeral, window, cx).is_empty());
            assert!(
                !cx.global::<InputOverlayRegistry<crate::input::EditorMode>>()
                    .hosts
                    .contains_key(&ephemeral.entity_id())
            );
            let owner = ephemeral.downgrade();
            drop(ephemeral);
            owner
        });
        cx.run_until_parked();
        assert!(dropped_owner.upgrade().is_none());
    }

    /// Typing a query must keep every character, including the first.
    ///
    /// Opening search anchors nothing when the query did not change. The first
    /// character anchors the matcher to the viewport, and that anchor change
    /// used to look like a new search session: the next frame selected the
    /// field, so the following character replaced the one just typed.
    ///
    /// Each character is its own update, with a frame after the query write,
    /// the same split as a real keystroke. One update that types the whole
    /// word hides the bug, because the anchor is set and cleared again before
    /// the overlay looks.
    #[gpui::test]
    fn typing_a_search_query_keeps_the_first_character(cx: &mut gpui::TestAppContext) {
        use crate::input::Editor;
        use gpui::{Bounds, WindowBounds, WindowOptions, px, size};

        struct SearchProbe {
            state: Entity<crate::input::EditorState>,
        }

        impl Render for SearchProbe {
            fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .child(Editor::new(&self.state).size_full())
            }
        }

        cx.update(crate::init);
        let mut editor = None;
        let window = cx
            .update(|cx| {
                cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds {
                            origin: Default::default(),
                            size: size(px(800.), px(480.)),
                        })),
                        ..Default::default()
                    },
                    |window, cx| {
                        let state = cx.new(|cx| {
                            crate::input::EditorState::new(window, cx)
                                .default_value("{\"host\": \"a.test\"}")
                        });
                        editor = Some(state.clone());
                        let probe = cx.new(|_| SearchProbe { state });
                        cx.new(|cx| crate::Root::new(probe, window, cx))
                    },
                )
            })
            .unwrap();
        let state = editor.unwrap();
        let mut cx = gpui::VisualTestContext::from_window(window.into(), cx);

        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        cx.update(|_window, cx| {
            state.update(cx, |state, cx| state.open_search(false, cx));
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        let search_input = cx.update(|_, cx| {
            cx.global::<InputOverlayRegistry<crate::input::EditorMode>>()
                .hosts
                .get(&state.entity_id())
                .expect("open search registers an overlay host")
                .1
                .search
                .clone()
        });
        let search_input = search_input.read_with(&cx, |panel, _| panel.search_input().clone());
        cx.update(|window, cx| {
            search_input.update(cx, |input, cx| input.focus(window, cx));
        });

        let mut expected = String::new();
        for ch in ['h', 'o', 's', 't'] {
            cx.update(|window, cx| {
                let text = ch.to_string();
                let mut key = gpui::Keystroke::parse_with_os(&text, cx.platform_info().os)
                    .expect("a letter is a keystroke");
                key.key_char = Some(text.into());
                window.dispatch_keystroke(key, cx);
            });
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
            });
            expected.push(ch);
            search_input.read_with(&cx, |input, _| {
                assert_eq!(
                    input.value().to_string(),
                    expected,
                    "the search field must keep every character typed so far"
                );
            });
        }
        state.read_with(&cx, |state, _| {
            assert_eq!(state.search_session().query, "host");
        });
    }
}
