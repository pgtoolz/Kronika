use super::Action;
use super::episode_actions;

#[test]
fn an_event_episode_has_one_of_each_showcase_action() {
    assert_eq!(
        episode_actions(),
        [Action::SlowQuery, Action::BadStatement, Action::BadDatabase]
    );
}
