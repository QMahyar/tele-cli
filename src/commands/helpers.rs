use grammers_client::tl;

pub(crate) fn peer_id(peer: &tl::enums::Peer) -> i64 {
    match peer {
        tl::enums::Peer::User(p) => p.user_id,
        tl::enums::Peer::Chat(p) => p.chat_id,
        tl::enums::Peer::Channel(p) => p.channel_id,
    }
}

#[allow(dead_code)]
pub(crate) fn peer_to_id(peer: &tl::enums::Peer) -> grammers_session::types::PeerId {
    match peer {
        tl::enums::Peer::User(p) => grammers_session::types::PeerId::user_unchecked(p.user_id),
        tl::enums::Peer::Chat(p) => grammers_session::types::PeerId::chat_unchecked(p.chat_id),
        tl::enums::Peer::Channel(p) => {
            grammers_session::types::PeerId::channel_unchecked(p.channel_id)
        }
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

pub(crate) fn looks_like_image(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "heic"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_period_json_shape() {
        let v = tl::enums::StatsDateRangeDays::Days(tl::types::StatsDateRangeDays {
            min_date: 100,
            max_date: 200,
        });
        assert_eq!(
            stats_period(&v),
            serde_json::json!({"min_date": 100, "max_date": 200})
        );
    }

    #[test]
    fn stats_abs_json_shape() {
        let v = tl::enums::StatsAbsValueAndPrev::Prev(tl::types::StatsAbsValueAndPrev {
            current: 12.5,
            previous: 10.0,
        });
        assert_eq!(
            stats_abs(&v),
            serde_json::json!({"current": 12.5, "previous": 10.0})
        );
    }

    #[test]
    fn stats_percent_json_shape() {
        let v = tl::enums::StatsPercentValue::Value(tl::types::StatsPercentValue {
            part: 50.0,
            total: 100.0,
        });
        assert_eq!(
            stats_percent(&v),
            serde_json::json!({"part": 50.0, "total": 100.0})
        );
    }

    #[test]
    fn looks_like_image_jpg_true() {
        assert!(looks_like_image("photo.jpg"));
    }

    #[test]
    fn looks_like_image_bmp_true() {
        assert!(looks_like_image("photo.bmp"));
    }

    #[test]
    fn looks_like_image_txt_false() {
        assert!(!looks_like_image("photo.txt"));
    }

    #[test]
    fn looks_like_image_case_insensitive() {
        assert!(looks_like_image("photo.JPG"));
        assert!(looks_like_image("photo.JpEg"));
        assert!(looks_like_image("PHOTO.PNG"));
        assert!(!looks_like_image("photo.TXT"));
    }
}
