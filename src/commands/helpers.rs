use grammers_client::tl;

pub(crate) fn peer_id(peer: &tl::enums::Peer) -> i64 {
    match peer {
        tl::enums::Peer::User(p) => p.user_id,
        tl::enums::Peer::Chat(p) => p.chat_id,
        tl::enums::Peer::Channel(p) => p.channel_id,
    }
}

pub(crate) fn stats_period(v: &tl::enums::StatsDateRangeDays) -> serde_json::Value {
    match v {
        tl::enums::StatsDateRangeDays::Days(d) => {
            serde_json::json!({"min_date": d.min_date, "max_date": d.max_date})
        }
    }
}

pub(crate) fn stats_abs(v: &tl::enums::StatsAbsValueAndPrev) -> serde_json::Value {
    match v {
        tl::enums::StatsAbsValueAndPrev::Prev(p) => {
            serde_json::json!({"current": p.current, "previous": p.previous})
        }
    }
}

pub(crate) fn stats_percent(v: &tl::enums::StatsPercentValue) -> serde_json::Value {
    match v {
        tl::enums::StatsPercentValue::Value(p) => {
            serde_json::json!({"part": p.part, "total": p.total})
        }
    }
}
