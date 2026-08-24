use clap::{Args, Subcommand};
use grammers_client::tl;

use crate::client::{self, ClientGuard};
use crate::commands::credentials::creds_api_id;
use crate::error::{tele_invocation, TeleError, TeleResult};
use crate::executor::{require_explicit_selection, run_fanout, GlobalFlags};
use crate::output;

const SET_COLUMNS: [&str; 7] = [
    "short_name",
    "title",
    "count",
    "official",
    "masks",
    "archived",
    "installed",
];

#[derive(Subcommand)]
pub enum StickerCmd {
    List(ListArgs),
    Search(SearchArgs),
    Show(ShowArgs),
    Install(InstallArgs),
    Remove(RemoveArgs),
}

#[derive(Args)]
pub struct ListArgs {
    #[arg(long, default_value_t = 200, help = "max sets to list (1-10000)")]
    limit: u32,
}

#[derive(Args)]
pub struct SearchArgs {
    #[arg(long, help = "search query (set title or keyword)")]
    query: String,
    #[arg(long, default_value_t = 50, help = "max results to show (1-10000)")]
    limit: u32,
}

#[derive(Args)]
pub struct ShowArgs {
    #[arg(long, help = "sticker-set short name or t.me/addstickers link")]
    set: String,
}

#[derive(Args)]
pub struct InstallArgs {
    #[arg(long, help = "sticker-set short name or t.me/addstickers link")]
    set: String,
    #[arg(
        long,
        help = "install into the archived section instead of the active tray"
    )]
    archive: bool,
}

#[derive(Args)]
pub struct RemoveArgs {
    #[arg(long, help = "sticker-set short name or t.me/addstickers link")]
    set: String,
}

pub async fn run(cmd: StickerCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        StickerCmd::List(a) => list(a, flags).await,
        StickerCmd::Search(a) => search(a, flags).await,
        StickerCmd::Show(a) => show(a, flags).await,
        StickerCmd::Install(a) => install(a, flags).await,
        StickerCmd::Remove(a) => remove(a, flags).await,
    }
}

fn parse_set_ref(raw: &str) -> TeleResult<String> {
    let mut s = raw.trim();
    if s.is_empty() {
        return Err(TeleError::Usage("--set must not be empty".to_string()));
    }
    let lower = s.to_ascii_lowercase();
    for prefix in ["https://", "http://"] {
        if lower.starts_with(prefix) {
            s = &s[prefix.len()..];
            break;
        }
    }
    let lower = s.to_ascii_lowercase();
    if let Some(pos) = lower.find("t.me/addstickers/") {
        s = &s[pos + "t.me/addstickers/".len()..];
    } else if lower.contains("t.me/") || s.contains('/') {
        return Err(TeleError::Usage(format!(
            "--set \"{raw}\" must be a sticker-set short name or a t.me/addstickers link"
        )));
    }
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(TeleError::Usage(format!(
            "--set \"{raw}\" must be a sticker-set short name (letters, digits, underscore) or a t.me/addstickers link"
        )));
    }
    if s.len() > 64 {
        return Err(TeleError::Usage(format!(
            "--set \"{raw}\" is longer than 64 characters"
        )));
    }
    Ok(s.to_string())
}

fn validate_query(query: &str) -> TeleResult<()> {
    if query.trim().is_empty() {
        return Err(TeleError::Usage(
            "--query must not be empty; pass a title or keyword to search".to_string(),
        ));
    }
    Ok(())
}

fn input_set(short_name: &str) -> tl::enums::InputStickerSet {
    tl::enums::InputStickerSet::ShortName(tl::types::InputStickerSetShortName {
        short_name: short_name.to_string(),
    })
}

fn set_row(set: &tl::types::StickerSet) -> serde_json::Value {
    serde_json::json!({
        "short_name": set.short_name,
        "title": set.title,
        "count": set.count,
        "official": set.official,
        "masks": set.masks,
        "archived": set.archived,
        "installed": set.installed_date.is_some(),
    })
}

fn covered_row(covered: &tl::enums::StickerSetCovered) -> serde_json::Value {
    match covered.set() {
        tl::enums::StickerSet::Set(set) => set_row(&set),
    }
}

fn rows_from_all(resp: tl::enums::messages::AllStickers) -> Vec<serde_json::Value> {
    match resp {
        tl::enums::messages::AllStickers::NotModified => Vec::new(),
        tl::enums::messages::AllStickers::Stickers(all) => all
            .sets
            .iter()
            .map(|s| {
                let tl::enums::StickerSet::Set(set) = s;
                set_row(set)
            })
            .collect(),
    }
}

fn rows_from_found(resp: tl::enums::messages::FoundStickerSets) -> Vec<serde_json::Value> {
    match resp {
        tl::enums::messages::FoundStickerSets::NotModified => Vec::new(),
        tl::enums::messages::FoundStickerSets::Sets(found) => {
            found.sets.iter().map(covered_row).collect()
        }
    }
}

fn install_report(
    resp: tl::enums::messages::StickerSetInstallResult,
    short_name: &str,
) -> serde_json::Value {
    let archived_sets: Vec<String> = match &resp {
        tl::enums::messages::StickerSetInstallResult::Success => Vec::new(),
        tl::enums::messages::StickerSetInstallResult::Archive(a) => a
            .sets
            .iter()
            .map(|c| {
                let tl::enums::StickerSet::Set(s) = c.set();
                s.short_name
            })
            .collect(),
    };
    serde_json::json!({
        "set": short_name,
        "installed": true,
        "archived_sets": archived_sets,
    })
}

fn table_row(row: &serde_json::Value) -> Vec<String> {
    vec![
        row["short_name"].as_str().unwrap_or_default().to_string(),
        row["title"].as_str().unwrap_or_default().to_string(),
        row["count"].to_string(),
        row["official"].to_string(),
        row["masks"].to_string(),
        row["archived"].to_string(),
        row["installed"].to_string(),
    ]
}

fn list_dry_run_payload() -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "would": "list installed sticker sets"
    })
}

fn search_dry_run_payload(query: &str) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "query": query,
        "would": format!("search sticker sets matching \"{query}\"")
    })
}

fn show_dry_run_payload(short_name: &str) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "set": short_name,
        "would": format!("fetch sticker set {short_name}")
    })
}

fn install_dry_run_payload(short_name: &str, archive: bool) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "set": short_name,
        "archive": archive,
        "would": format!("install sticker set {short_name}")
    })
}

fn remove_dry_run_payload(short_name: &str) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "set": short_name,
        "would": format!("remove (uninstall) sticker set {short_name}")
    })
}

async fn list(args: ListArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let limit = args.limit as usize;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(list_dry_run_payload());
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let resp: tl::enums::messages::AllStickers = guard
                .client
                .invoke(&tl::functions::messages::GetAllStickers { hash: 0 })
                .await
                .map_err(tele_invocation)?;
            let mut rows = rows_from_all(resp);
            rows.truncate(limit);
            if !output::machine_mode(json, jsonl) {
                let table_rows: Vec<Vec<String>> = rows.iter().map(table_row).collect();
                output::print_account_table(&name, multi, &SET_COLUMNS, &table_rows)?;
            }
            Ok(serde_json::json!({ "sets": rows }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn search(args: SearchArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    validate_query(&args.query)?;
    crate::commands::validate_limit(args.limit, 10_000, "limit")?;
    let query = args.query.trim().to_string();
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let limit = args.limit as usize;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let query = query.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(search_dry_run_payload(&query));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let resp: tl::enums::messages::FoundStickerSets = guard
                .client
                .invoke(&tl::functions::messages::SearchStickerSets {
                    exclude_featured: false,
                    q: query.clone(),
                    hash: 0,
                })
                .await
                .map_err(tele_invocation)?;
            let mut rows = rows_from_found(resp);
            rows.truncate(limit);
            if !output::machine_mode(json, jsonl) {
                let table_rows: Vec<Vec<String>> = rows.iter().map(table_row).collect();
                output::print_account_table(&name, multi, &SET_COLUMNS, &table_rows)?;
            }
            Ok(serde_json::json!({ "query": query, "sets": rows }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn show(args: ShowArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let short_name = parse_set_ref(&args.set)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let short_name = short_name.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(show_dry_run_payload(&short_name));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let resp: tl::enums::messages::StickerSet = guard
                .client
                .invoke(&tl::functions::messages::GetStickerSet {
                    stickerset: input_set(&short_name),
                    hash: 0,
                })
                .await
                .map_err(tele_invocation)?;
            let full = match resp {
                tl::enums::messages::StickerSet::Set(full) => full,
                tl::enums::messages::StickerSet::NotModified => {
                    return Err(TeleError::Other(format!(
                        "sticker set {short_name}: server reported no change (unexpected without a cache hash)"
                    )));
                }
            };
            let tl::enums::StickerSet::Set(set) = full.set;
            let mut row = set_row(&set);
            row["documents"] = serde_json::json!(full.documents.len());
            if !output::machine_mode(json, jsonl) {
                let mut columns: Vec<&str> = SET_COLUMNS.to_vec();
                columns.insert(3, "documents");
                let cells: Vec<String> = vec![
                    row["short_name"].as_str().unwrap_or_default().to_string(),
                    row["title"].as_str().unwrap_or_default().to_string(),
                    row["count"].to_string(),
                    row["documents"].to_string(),
                    row["official"].to_string(),
                    row["masks"].to_string(),
                    row["archived"].to_string(),
                    row["installed"].to_string(),
                ];
                output::print_account_table(&name, multi, &columns, &[cells])?;
            }
            Ok(row)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn install(args: InstallArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let short_name = parse_set_ref(&args.set)?;
    require_explicit_selection("sticker install", flags)?;
    let archive = args.archive;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let short_name = short_name.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(install_dry_run_payload(&short_name, archive));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let resp: tl::enums::messages::StickerSetInstallResult = guard
                .client
                .invoke(&tl::functions::messages::InstallStickerSet {
                    stickerset: input_set(&short_name),
                    archived: archive,
                })
                .await
                .map_err(tele_invocation)?;
            let mut report = install_report(resp, &short_name);
            report["archive"] = serde_json::json!(archive);
            if !output::machine_mode(json, jsonl) {
                let line = if archive {
                    format!("installed sticker set {short_name} (archived)")
                } else {
                    format!("installed sticker set {short_name}")
                };
                let line = if multi {
                    format!("{name}: {line}")
                } else {
                    line
                };
                output::print_line(&line)?;
            }
            Ok(report)
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

async fn remove(args: RemoveArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let short_name = parse_set_ref(&args.set)?;
    require_explicit_selection("sticker remove", flags)?;
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;
    let jsonl = flags.jsonl;
    let multi = crate::executor::select_accounts(flags)?.len() > 1;
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let short_name = short_name.clone();
        Box::pin(async move {
            if dry_run {
                return Ok(remove_dry_run_payload(&short_name));
            }
            let guard =
                ClientGuard::connect(&name, creds_api_id()?, config_path.as_deref()).await?;
            client::authorize(&guard.client).await?;
            guard.rate_limiter.acquire().await;
            let removed: bool = guard
                .client
                .invoke(&tl::functions::messages::UninstallStickerSet {
                    stickerset: input_set(&short_name),
                })
                .await
                .map_err(tele_invocation)?;
            if !removed {
                return Err(TeleError::Other(format!(
                    "Telegram returned false uninstalling sticker set {short_name}"
                )));
            }
            if !output::machine_mode(json, jsonl) {
                let line = format!("removed sticker set {short_name}");
                let line = if multi {
                    format!("{name}: {line}")
                } else {
                    line
                };
                output::print_line(&line)?;
            }
            Ok(serde_json::json!({ "set": short_name, "removed": true }))
        })
    })
    .await?;
    crate::executor::finish(flags, &envelope)
}

pub(crate) fn stickers_serve_routes() -> Vec<crate::commands::serve::OpRoute> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EXIT_USAGE;

    fn fake_sticker_set(
        short_name: &str,
        installed_date: Option<i32>,
        archived: bool,
        official: bool,
        masks: bool,
        count: i32,
    ) -> tl::types::StickerSet {
        tl::types::StickerSet {
            archived,
            official,
            masks,
            emojis: false,
            text_color: false,
            channel_emoji_status: false,
            creator: false,
            installed_date,
            id: 1,
            access_hash: 2,
            title: format!("Title of {short_name}"),
            short_name: short_name.to_string(),
            thumbs: None,
            thumb_dc_id: None,
            thumb_version: None,
            thumb_document_id: None,
            count,
            hash: 0,
        }
    }

    fn fake_enum_set(
        short_name: &str,
        installed_date: Option<i32>,
        archived: bool,
    ) -> tl::enums::StickerSet {
        tl::enums::StickerSet::Set(fake_sticker_set(
            short_name,
            installed_date,
            archived,
            false,
            false,
            3,
        ))
    }

    #[test]
    fn parse_accepts_plain_short_names() {
        for good in ["ducks", "Duck_S2", "a1_b2"] {
            assert_eq!(parse_set_ref(good).unwrap(), good, "for {good}");
        }
    }

    #[test]
    fn parse_trims_whitespace() {
        assert_eq!(parse_set_ref("  ducks \n").unwrap(), "ducks");
    }

    #[test]
    fn parse_extracts_names_from_addstickers_links() {
        assert_eq!(parse_set_ref("t.me/addstickers/ducks").unwrap(), "ducks");
        assert_eq!(
            parse_set_ref("https://t.me/addstickers/ducks").unwrap(),
            "ducks"
        );
        assert_eq!(
            parse_set_ref("http://t.me/addstickers/Duck_S2").unwrap(),
            "Duck_S2"
        );
    }

    #[test]
    fn parse_rejects_empty_refs() {
        for bad in ["", "   "] {
            let err = parse_set_ref(bad).unwrap_err();
            assert!(matches!(err, TeleError::Usage(_)), "for {bad:?}");
            assert_eq!(err.exit_code(), EXIT_USAGE);
            assert!(err.message().contains("--set"), "for {bad:?}");
        }
    }

    #[test]
    fn parse_rejects_empty_link_target() {
        for bad in ["https://t.me/addstickers/", "https://t.me/addstickers"] {
            let err = parse_set_ref(bad).unwrap_err();
            assert!(matches!(err, TeleError::Usage(_)), "for {bad}");
        }
    }

    #[test]
    fn parse_rejects_paths_and_invalid_chars() {
        for bad in [
            "bad name!",
            "a/b",
            "дюкс",
            "t.me/addstickers/ducks/extra",
            "t.me/somechannel",
            "ducks?x=1",
        ] {
            let err = parse_set_ref(bad).unwrap_err();
            assert!(matches!(err, TeleError::Usage(_)), "for {bad}");
            assert!(
                err.message().contains("short name"),
                "{bad}: {}",
                err.message()
            );
        }
    }

    #[test]
    fn parse_rejects_oversized_names() {
        let long = "a".repeat(65);
        let err = parse_set_ref(&long).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert!(parse_set_ref(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn input_set_wraps_short_name() {
        let input = input_set("ducks");
        match input {
            tl::enums::InputStickerSet::ShortName(s) => assert_eq!(s.short_name, "ducks"),
            other => panic!("expected ShortName, got {other:?}"),
        }
    }

    #[test]
    fn set_row_carries_flags_and_installed_presence() {
        let row = set_row(&fake_sticker_set(
            "ducks",
            Some(1700000000),
            true,
            true,
            false,
            120,
        ));
        assert_eq!(row["short_name"], serde_json::json!("ducks"));
        assert_eq!(row["title"], serde_json::json!("Title of ducks"));
        assert_eq!(row["count"], serde_json::json!(120));
        assert_eq!(row["official"], serde_json::json!(true));
        assert_eq!(row["masks"], serde_json::json!(false));
        assert_eq!(row["archived"], serde_json::json!(true));
        assert_eq!(row["installed"], serde_json::json!(true));

        let row = set_row(&fake_sticker_set("plain", None, false, false, true, 7));
        assert_eq!(row["installed"], serde_json::json!(false));
        assert_eq!(row["archived"], serde_json::json!(false));
        assert_eq!(row["masks"], serde_json::json!(true));
        assert_eq!(row["count"], serde_json::json!(7));
    }

    #[test]
    fn covered_row_reads_the_nested_set_across_variants() {
        let no_cover =
            tl::enums::StickerSetCovered::StickerSetNoCovered(tl::types::StickerSetNoCovered {
                set: fake_enum_set("nocover", None, false),
            });
        let row = covered_row(&no_cover);
        assert_eq!(row["short_name"], serde_json::json!("nocover"));
        assert_eq!(row["count"], serde_json::json!(3));

        let multi = tl::enums::StickerSetCovered::StickerSetMultiCovered(
            tl::types::StickerSetMultiCovered {
                set: fake_enum_set("multicover", Some(5), false),
                covers: vec![tl::enums::Document::Empty(tl::types::DocumentEmpty {
                    id: 0,
                })],
            },
        );
        let row = covered_row(&multi);
        assert_eq!(row["short_name"], serde_json::json!("multicover"));
        assert_eq!(row["installed"], serde_json::json!(true));

        let full =
            tl::enums::StickerSetCovered::StickerSetFullCovered(tl::types::StickerSetFullCovered {
                set: fake_enum_set("fullcover", None, true),
                packs: vec![],
                keywords: vec![],
                documents: vec![],
            });
        let row = covered_row(&full);
        assert_eq!(row["archived"], serde_json::json!(true));

        let single = tl::enums::StickerSetCovered::Covered(tl::types::StickerSetCovered {
            set: fake_enum_set("single", None, false),
            cover: tl::enums::Document::Empty(tl::types::DocumentEmpty { id: 0 }),
        });
        assert_eq!(
            covered_row(&single)["short_name"],
            serde_json::json!("single")
        );
    }

    #[test]
    fn rows_from_all_shapes_installed_sets_and_handles_not_modified() {
        let resp = tl::enums::messages::AllStickers::Stickers(tl::types::messages::AllStickers {
            hash: 0,
            sets: vec![
                fake_enum_set("active", Some(1), false),
                fake_enum_set("archived_pack", None, true),
            ],
        });
        let rows = rows_from_all(resp);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["short_name"], serde_json::json!("active"));
        assert_eq!(rows[0]["installed"], serde_json::json!(true));
        assert_eq!(rows[1]["short_name"], serde_json::json!("archived_pack"));
        assert_eq!(rows[1]["archived"], serde_json::json!(true));

        assert!(rows_from_all(tl::enums::messages::AllStickers::NotModified).is_empty());
    }

    #[test]
    fn rows_from_found_shapes_search_hits_and_handles_not_modified() {
        let resp =
            tl::enums::messages::FoundStickerSets::Sets(tl::types::messages::FoundStickerSets {
                hash: 0,
                sets: vec![tl::enums::StickerSetCovered::StickerSetNoCovered(
                    tl::types::StickerSetNoCovered {
                        set: fake_enum_set("hit", None, false),
                    },
                )],
            });
        let rows = rows_from_found(resp);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["short_name"], serde_json::json!("hit"));

        assert!(rows_from_found(tl::enums::messages::FoundStickerSets::NotModified).is_empty());
    }

    #[test]
    fn install_report_distinguishes_success_from_auto_archive() {
        let ok = install_report(
            tl::enums::messages::StickerSetInstallResult::Success,
            "ducks",
        );
        assert_eq!(ok["set"], serde_json::json!("ducks"));
        assert_eq!(ok["installed"], serde_json::json!(true));
        assert_eq!(ok["archived_sets"], serde_json::json!([]));

        let archived = install_report(
            tl::enums::messages::StickerSetInstallResult::Archive(
                tl::types::messages::StickerSetInstallResultArchive {
                    sets: vec![tl::enums::StickerSetCovered::StickerSetNoCovered(
                        tl::types::StickerSetNoCovered {
                            set: fake_enum_set("old_pack", None, false),
                        },
                    )],
                },
            ),
            "new_pack",
        );
        assert_eq!(archived["installed"], serde_json::json!(true));
        assert_eq!(archived["archived_sets"], serde_json::json!(["old_pack"]));
    }

    #[test]
    fn table_row_matches_column_count_and_order() {
        let row = set_row(&fake_sticker_set("ducks", Some(1), false, true, false, 12));
        let cells = table_row(&row);
        assert_eq!(cells.len(), SET_COLUMNS.len());
        assert_eq!(SET_COLUMNS[0], "short_name");
        assert_eq!(cells[0], "ducks");
        assert_eq!(cells[2], "12");
        assert_eq!(cells[3], "true");
        assert_eq!(cells[6], "true");
    }

    #[test]
    fn dry_run_payloads_carry_would_text_and_arguments() {
        let v = list_dry_run_payload();
        assert_eq!(v["dry_run"], serde_json::json!(true));
        assert_eq!(v["would"], serde_json::json!("list installed sticker sets"));

        let v = search_dry_run_payload("cats");
        assert_eq!(v["query"], serde_json::json!("cats"));
        assert_eq!(
            v["would"],
            serde_json::json!("search sticker sets matching \"cats\"")
        );

        let v = show_dry_run_payload("ducks");
        assert_eq!(v["set"], serde_json::json!("ducks"));
        assert_eq!(v["would"], serde_json::json!("fetch sticker set ducks"));

        let v = install_dry_run_payload("ducks", false);
        assert_eq!(v["archive"], serde_json::json!(false));
        assert_eq!(v["would"], serde_json::json!("install sticker set ducks"));

        let v = install_dry_run_payload("ducks", true);
        assert_eq!(v["archive"], serde_json::json!(true));

        let v = remove_dry_run_payload("ducks");
        assert_eq!(v["set"], serde_json::json!("ducks"));
        assert_eq!(
            v["would"],
            serde_json::json!("remove (uninstall) sticker set ducks")
        );

        for v in [
            list_dry_run_payload(),
            search_dry_run_payload("q"),
            show_dry_run_payload("s"),
            install_dry_run_payload("s", false),
            remove_dry_run_payload("s"),
        ] {
            assert_eq!(v["dry_run"], serde_json::json!(true));
        }
    }

    #[test]
    fn empty_query_is_a_usage_error() {
        let err = validate_query("").unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
        assert_eq!(err.exit_code(), EXIT_USAGE);
        assert!(err.message().contains("--query"));

        assert!(validate_query("cats").is_ok());
        assert!(validate_query("  cats ").is_ok());

        let err = validate_query("   ").unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)));
    }
}

