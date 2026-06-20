// NOTICE provenance gate (spec §3): the credits must never silently drop.
#[test]
fn notice_credits_all_required_upstreams() {
    let notice = include_str!("../../../NOTICE");
    for who in ["Hermes Agent", "EvoMap", "Honcho", "agentskills.io"] {
        assert!(notice.contains(who), "NOTICE must credit {who}");
    }
    assert!(
        notice.contains("clean-room") || notice.contains("No Hermes source"),
        "NOTICE must state clean-room provenance"
    );
}
