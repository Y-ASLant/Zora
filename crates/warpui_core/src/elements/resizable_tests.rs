use super::{DragBarSide, ResizableState};
use pathfinder_geometry::vector::vec2f;

#[test]
fn temporary_bounds_do_not_overwrite_requested_size() {
    let mut state = ResizableState::new(376.);

    state.set_bounds((200., 200.));
    assert_eq!(state.effective_size(), 200.);
    assert_eq!(state.requested_size(), 376.);

    state.set_bounds((200., 600.));
    assert_eq!(state.effective_size(), 376.);
    assert_eq!(state.requested_size(), 376.);
}

#[test]
fn setting_size_updates_requested_size_while_respecting_current_bounds() {
    let mut state = ResizableState::new(248.);
    state.set_bounds((200., 300.));

    state.set_requested_size(376.);
    assert_eq!(state.effective_size(), 300.);
    assert_eq!(state.requested_size(), 376.);

    state.set_bounds((200., 600.));
    assert_eq!(state.effective_size(), 376.);
}

#[test]
fn constrained_drag_does_not_commit_a_temporary_effective_size() {
    let mut state = ResizableState::new(376.);
    state.set_bounds((200., 200.));
    state.begin_resizing(vec2f(200., 0.));

    assert_eq!(
        state.check_for_resize(
            vec2f(220., 0.),
            Some(vec2f(0., 0.)),
            DragBarSide::Right,
        ),
        None
    );
    assert_eq!(state.effective_size(), 200.);
    assert_eq!(state.requested_size(), 376.);
}
