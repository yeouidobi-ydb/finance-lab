//! KRX 휴장일 및 장운영 관련 공지 조회 (krx_schedule.py와 같은 동작)
//! - 휴장일: open.krx.co.kr GenerateOTP + OPN99000001.jspx
//!   연도별로 받아 data/krx_holidays_<연도>.json 에 저장하고, 이후에는 저장본을 쓴다.
//!   올해 이후 연도의 저장본은 매주 월·목 0시(KST)가 지나면 처음 조회할 때 다시 받는다.
//!   (Python 버전과 같은 파일을 같이 쓴다. 위치는 KRX_SCHEDULE_DATA_DIR 로 바꿀 수 있다.)
//! - 참고 공지: strn.krx.co.kr/corebbs5/BHPSTRN0401/list
//!   주의: 'KRX 개인 공매도 모의거래인증시스템' 게시판이다. 정규장 공지가 아니므로 참고용.
//!
//! 사용법:
//!   krx_schedule [YYYY-MM-DD]    날짜 조회 (생략 시 오늘, KST). 그 해 저장본이 없으면 받아서 저장
//!   krx_schedule --update [YYYY] 그 해 휴장일을 다시 받아 저장본 교체 (생략 시 올해, KST)
//!
//! 테스트: cargo test   (네트워크 호출 없음. krx_schedule.py 의 test_krx_schedule.py 와 같은 경우를 확인)

use chrono::{DateTime, Datelike, Duration as ChronoDuration, FixedOffset, NaiveDate, Utc};
use regex::Regex;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

const UA: &str = "Mozilla/5.0";
const OTP_URL: &str =
    "https://open.krx.co.kr/contents/COM/GenerateOTP.jspx?name=form&bld=MKD/01/0110/01100305/mkd01100305_01";
const HOLIDAY_URL: &str = "https://open.krx.co.kr/contents/OPN/99/OPN99000001.jspx";
const STRN_LIST_URL: &str = "https://strn.krx.co.kr/corebbs5/BHPSTRN0401/list";
const KEYWORDS: [&str; 8] = ["수능", "임시", "휴장", "시간", "개장", "대체", "선거", "변경"];
const DATA_DIR_ENV: &str = "KRX_SCHEDULE_DATA_DIR";
const USAGE: &str = "사용법: krx_schedule [YYYY-MM-DD] | krx_schedule --update [YYYY]";

const NOTE_CACHED: &str = "저장본 사용";
const NOTE_NEW: &str = "방금 새로 받아 저장";
const NOTE_REFRESHED: &str = "저장본이 오래돼 다시 받아 저장";
const NOTE_PREVIEW: &str = "다음 해 이후라 저장 안 함 · 확정 전일 수 있음";

type BoxResult<T> = Result<T, Box<dyn Error>>;

#[derive(Debug, Deserialize)]
struct HolidayResponse {
    #[serde(default)]
    block1: Option<Vec<RawHoliday>>,
}

/// KRX 응답 그대로 (필드가 비거나 null일 수 있음)
#[derive(Debug, Deserialize)]
struct RawHoliday {
    #[serde(default)]
    calnd_dd: Option<String>,
    #[serde(default)]
    kr_dy_tp: Option<String>,
    #[serde(default)]
    holdy_nm: Option<String>,
}

/// 저장본에 들어가는 형태. Python과 같이 쓰므로 세 필드 모두 문자열.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
struct Holiday {
    calnd_dd: String,
    kr_dy_tp: String,
    holdy_nm: String,
}

impl Holiday {
    fn day_name(&self) -> &str {
        &self.kr_dy_tp
    }
    fn name(&self) -> &str {
        &self.holdy_nm
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct HolidayCache {
    year: i32,
    fetched_at: String,
    #[serde(default)]
    source: String,
    holidays: Vec<Holiday>,
}

#[derive(Debug, Clone)]
struct Notice {
    posted_at: String,
    title: String,
    url: String,
}

fn kst() -> FixedOffset {
    FixedOffset::east_opt(9 * 3600).expect("valid offset")
}

fn now_kst() -> DateTime<FixedOffset> {
    Utc::now().with_timezone(&kst())
}

fn today_kst() -> NaiveDate {
    now_kst().date_naive()
}

fn kst_midnight(day: NaiveDate) -> DateTime<FixedOffset> {
    day.and_hms_opt(0, 0, 0)
        .expect("valid time")
        .and_local_timezone(kst())
        .single()
        .expect("fixed offset is unambiguous")
}

fn fetch_holidays(client: &Client, year: i32) -> BoxResult<Vec<Holiday>> {
    let code = client
        .get(OTP_URL)
        .header("User-Agent", UA)
        .send()?
        .error_for_status()?
        .text()?
        .trim()
        .to_string();
    if code.is_empty() {
        return Err("OTP 발급 실패: 빈 응답".into());
    }

    let params = [
        ("search_bas_yy", year.to_string()),
        ("gridTp", "KRX".to_string()),
        (
            "pagePath",
            "/contents/MKD/01/0110/01100305/MKD01100305.jsp".to_string(),
        ),
        ("code", code),
        ("pageFirstCall", "Y".to_string()),
    ];

    let raw = client
        .post(HOLIDAY_URL)
        .header("User-Agent", UA)
        .form(&params)
        .send()?
        .error_for_status()?
        .text()?;
    parse_holidays(&raw, year)
}

/// KRX 휴장일 응답(JSON 문자열)을 저장 형식으로 바꾼다. 이상하면 오류.
fn parse_holidays(raw: &str, year: i32) -> BoxResult<Vec<Holiday>> {
    let resp: HolidayResponse = serde_json::from_str(raw).map_err(|e| {
        let head: String = raw.chars().take(200).collect();
        format!("휴장일 응답이 JSON이 아님 ({e}): {head:?}")
    })?;

    // 매년 최소 신정·연말휴장일이 있으므로 비어 있으면 조회 실패로 본다.
    let block1 = resp.block1.unwrap_or_default();
    if block1.is_empty() {
        return Err(format!("{year}년 휴장일 목록이 비어 있음 (조회 실패 가능성)").into());
    }
    // 저장본을 Python과 같이 쓰므로 세 필드만 문자열로 남긴다.
    let holidays: Vec<Holiday> = block1
        .into_iter()
        .map(|h| Holiday {
            calnd_dd: h.calnd_dd.unwrap_or_default(),
            kr_dy_tp: h.kr_dy_tp.unwrap_or_default(),
            holdy_nm: h.holdy_nm.unwrap_or_default(),
        })
        .collect();
    // 날짜가 비었거나 다른 해 날짜가 섞이면 응답을 믿지 않는다 (기존 저장본도 덮어쓰지 않음).
    let bad: Vec<&Holiday> = holidays
        .iter()
        .filter(|h| !valid_holiday_date(&h.calnd_dd, year))
        .take(3)
        .collect();
    if !bad.is_empty() {
        return Err(format!("{year}년 휴장일 응답에 이상한 항목이 있음: {bad:?}").into());
    }
    Ok(holidays)
}

fn valid_holiday_date(value: &str, year: i32) -> bool {
    value.len() == 10
        && NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok_and(|d| d.year() == year)
}

/// fetched_at 은 "2026-11-22T21:43:32+09:00" 형식만 받는다 (Python과 같은 규칙).
fn parse_fetched_at(value: &str) -> Option<DateTime<FixedOffset>> {
    let re = Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}[+-]\d{2}:\d{2}$").ok()?;
    if !re.is_match(value) {
        return None;
    }
    DateTime::parse_from_rfc3339(value).ok()
}

/// now 이전(같은 시각 포함)의 가장 가까운 월요일 또는 목요일 0시 (KST).
fn last_refresh_point(now: DateTime<FixedOffset>) -> DateTime<FixedOffset> {
    let now_kst = now.with_timezone(&kst());
    let days_back = [0, 1, 2, 0, 1, 2, 3][now_kst.weekday().num_days_from_monday() as usize];
    kst_midnight(now_kst.date_naive() - ChronoDuration::days(days_back))
}

/// 저장본을 다시 받아야 하는지.
/// - 올해 이후 연도: 마지막 월·목 0시(KST)보다 전에 받았으면 다시 받는다.
///   (해 중간에 정해지는 임시공휴일·선거일을 늦어도 3~4일 안에 반영)
/// - 지난 연도: 그 해가 끝나기 전에 받았으면 한 번 더 받는다. 그 뒤로는 바뀌지 않는다.
fn needs_refresh(year: i32, fetched_at: DateTime<FixedOffset>, now: DateTime<FixedOffset>) -> bool {
    if year < now.with_timezone(&kst()).year() {
        let next_year = NaiveDate::from_ymd_opt(year + 1, 1, 1).expect("valid date");
        return fetched_at < kst_midnight(next_year);
    }
    fetched_at < last_refresh_point(now)
}

fn data_dir() -> PathBuf {
    match env::var_os(DATA_DIR_ENV) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        // 실행 파일 위치는 빌드 방식마다 달라서, 소스 폴더(= Python 파일 옆) 기준으로 둔다.
        _ => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data"),
    }
}

fn cache_path(dir: &Path, year: i32) -> PathBuf {
    dir.join(format!("krx_holidays_{year}.json"))
}

fn save_holidays(
    dir: &Path,
    year: i32,
    holidays: Vec<Holiday>,
    now: DateTime<FixedOffset>,
) -> BoxResult<HolidayCache> {
    if holidays.is_empty() {
        return Err(format!("{year}년 휴장일 목록이 비어 있어 저장하지 않음").into());
    }
    let cache = HolidayCache {
        year,
        fetched_at: now
            .with_timezone(&kst())
            .format("%Y-%m-%dT%H:%M:%S%:z")
            .to_string(),
        source: HOLIDAY_URL.to_string(),
        holidays,
    };
    let path = cache_path(dir, year);
    fs::create_dir_all(dir)?;
    // 임시 파일은 프로세스마다 다른 이름 (Python·Rust를 동시에 돌려도 서로 덮어쓰지 않게)
    let tmp = dir.join(format!(
        "krx_holidays_{year}.json.{}.tmp",
        std::process::id()
    ));
    let json = serde_json::to_string_pretty(&cache)? + "\n";
    // 쓰다 끊겨도 기존 저장본이 깨지지 않게 임시 파일에 쓰고 바꿔 끼운다
    if let Err(e) = fs::write(&tmp, json).and_then(|()| fs::rename(&tmp, &path)) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(cache)
}

/// 저장본이 없으면 None. 있는데 이상하면 오류로 멈춘다.
fn load_cache(dir: &Path, year: i32) -> BoxResult<Option<HolidayCache>> {
    let path = cache_path(dir, year);
    if !path.exists() {
        return Ok(None);
    }
    let unreadable = |e: &dyn std::fmt::Display| {
        format!(
            "휴장일 저장본을 읽을 수 없음 ({}): {e} — --update {year} 로 다시 받으세요",
            path.display()
        )
    };
    let raw = fs::read_to_string(&path).map_err(|e| unreadable(&e))?;
    let cache: HolidayCache = serde_json::from_str(&raw).map_err(|e| unreadable(&e))?;

    if cache.year != year
        || parse_fetched_at(&cache.fetched_at).is_none()
        || cache.holidays.is_empty()
        || !cache
            .holidays
            .iter()
            .all(|h| valid_holiday_date(&h.calnd_dd, year))
    {
        return Err(format!(
            "휴장일 저장본 형식이 이상함 ({}) — --update {year} 로 다시 받으세요",
            path.display()
        )
        .into());
    }
    Ok(Some(cache))
}

/// (휴장일 데이터, 출처 설명).
/// - 저장본이 있고 오래되지 않았으면(needs_refresh) 그대로 쓴다.
/// - 오래됐으면 다시 받아 저장한다. 다시 받기에 실패하면 오래된 목록으로 넘기지 않고 오류로 멈춘다.
/// - 저장본이 없으면 받아서 저장한다. 단 올해(KST)보다 뒤의 연도는 KRX가 확정 전 달력
///   (연말휴장일 등이 빠짐)을 주므로 저장하지 않는다. 저장하려면 --update <연도>를 직접 돌린다.
fn get_holidays(
    dir: &Path,
    year: i32,
    now: DateTime<FixedOffset>,
    fetch: &mut dyn FnMut(i32) -> BoxResult<Vec<Holiday>>,
) -> BoxResult<(HolidayCache, &'static str)> {
    let now = now.with_timezone(&kst());
    if let Some(cache) = load_cache(dir, year)? {
        let fetched_at = parse_fetched_at(&cache.fetched_at).expect("load_cache 에서 확인함");
        if !needs_refresh(year, fetched_at, now) {
            return Ok((cache, NOTE_CACHED));
        }
        return Ok((save_holidays(dir, year, fetch(year)?, now)?, NOTE_REFRESHED));
    }

    let holidays = fetch(year)?;
    if year > now.year() {
        let preview = HolidayCache {
            year,
            fetched_at: now.format("%Y-%m-%dT%H:%M:%S%:z").to_string(),
            source: HOLIDAY_URL.to_string(),
            holidays,
        };
        return Ok((preview, NOTE_PREVIEW));
    }
    Ok((save_holidays(dir, year, holidays, now)?, NOTE_NEW))
}

fn fetch_notices(client: &Client, pages: usize) -> BoxResult<Vec<Notice>> {
    let re = Regex::new(
        r#"(?s)href="\./view\?bbsSeq=(\d+)"[^>]*>(.*?)</a>.*?(\d{4}-\d{2}-\d{2})"#,
    )?;
    let ws_re = Regex::new(r"\s+")?;

    let mut results = Vec::new();
    let mut seen = HashSet::new();

    for page in 1..=pages {
        let url = format!("{STRN_LIST_URL}?curPage={page}");
        let html = client
            .get(&url)
            .header("User-Agent", UA)
            .send()?
            .error_for_status()?
            .text()?;

        for cap in re.captures_iter(&html) {
            let bbs_seq = cap[1].to_string();
            if !seen.insert(bbs_seq.clone()) {
                continue;
            }
            let title = ws_re.replace_all(&cap[2], " ").trim().to_string();
            if KEYWORDS.iter().any(|k| title.contains(k)) {
                results.push(Notice {
                    posted_at: cap[3].to_string(),
                    title,
                    url: format!(
                        "https://strn.krx.co.kr/corebbs5/BHPSTRN0401/view?bbsSeq={bbs_seq}"
                    ),
                });
            }
        }
    }

    Ok(results)
}

fn date_tokens(target: NaiveDate) -> [String; 4] {
    [
        target.format("%Y-%m-%d").to_string(),
        target.format("%Y.%m.%d").to_string(),
        format!("{}년 {}월 {}일", target.year(), target.month(), target.day()),
        format!(
            "{}년 {:02}월 {:02}일",
            target.year(),
            target.month(),
            target.day()
        ),
    ]
}

fn build_client() -> BoxResult<Client> {
    Ok(Client::builder().timeout(Duration::from_secs(10)).build()?)
}

fn run_update(year: i32) -> BoxResult<()> {
    let client = build_client()?;
    let dir = data_dir();
    let cache = save_holidays(&dir, year, fetch_holidays(&client, year)?, now_kst())?;
    println!(
        "=== {}년 휴장일 저장 완료 ({}일, {} 받음) ===",
        year,
        cache.holidays.len(),
        cache.fetched_at
    );
    println!("- 저장 위치: {}", cache_path(&dir, year).display());
    for h in &cache.holidays {
        println!("  {} ({}) - {}", h.calnd_dd, h.day_name(), h.name());
    }
    Ok(())
}

fn run_check(target: NaiveDate) -> BoxResult<()> {
    let client = build_client()?;
    let (cache, data_note) = get_holidays(&data_dir(), target.year(), now_kst(), &mut |y| {
        fetch_holidays(&client, y)
    })?;
    let holidays = &cache.holidays;
    let notices = fetch_notices(&client, 1)?;

    let target_str = target.format("%Y-%m-%d").to_string();
    let is_weekend = target.weekday().num_days_from_monday() >= 5;
    let holiday_match = holidays.iter().find(|h| h.calnd_dd == target_str);

    let tokens = date_tokens(target);
    let notice_matches: Vec<&Notice> = notices
        .iter()
        .filter(|n| tokens.iter().any(|t| n.title.contains(t.as_str())))
        .collect();

    let verdict = if is_weekend {
        "휴장 (주말)".to_string()
    } else if let Some(h) = holiday_match {
        format!("휴장 ({})", h.name())
    } else {
        "개장".to_string()
    };

    println!("=== {} KRX 장운영 체크 ===", target_str);
    println!("- 판정: {}", verdict);
    println!("- 주말 여부: {}", if is_weekend { "True" } else { "False" });
    match holiday_match {
        Some(h) => println!("- 공식 휴장일: {} ({})", h.name(), h.day_name()),
        None => println!("- 공식 휴장일: 없음"),
    }
    println!(
        "- 휴장일 데이터: {} 받음 ({})",
        cache.fetched_at.get(..10).unwrap_or(&cache.fetched_at),
        data_note
    );
    println!(
        "- 참고 공지 · 공매도 모의거래 게시판 ({}건):",
        notice_matches.len()
    );
    for n in notice_matches {
        println!("  * [{}] {} ({})", n.posted_at, n.title, n.url);
    }

    println!("\n=== {}년 전체 휴장일 ===", target.year());
    for h in holidays {
        println!("  {} ({}) - {}", h.calnd_dd, h.day_name(), h.name());
    }

    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    let result = if args.first().map(String::as_str) == Some("--update") {
        let year = match &args[1..] {
            [] => now_kst().year(),
            [y] if y.len() == 4 && y.bytes().all(|b| b.is_ascii_digit()) => {
                y.parse().expect("4 digits")
            }
            _ => {
                eprintln!("[오류] 연도는 YYYY 형식 하나만 받음\n{USAGE}");
                return ExitCode::from(2);
            }
        };
        run_update(year)
    } else {
        let target = match args.as_slice() {
            [] => today_kst(),
            [d] => match NaiveDate::parse_from_str(d, "%Y-%m-%d") {
                Ok(t) => t,
                Err(_) => {
                    eprintln!("[오류] 날짜는 YYYY-MM-DD 형식\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        };
        run_check(target)
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[오류] KRX 조회 실패: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    //! test_krx_schedule.py 와 같은 경우를 확인한다 (네트워크 호출 없음).
    use super::*;
    use std::cell::Cell;

    // 실제 KRX 응답 모양 (저장하지 않는 필드 dy_tp_cd, calnd_dd_dy 포함)
    const SAMPLE_RESPONSE: &str = r#"{"block1":[
 {"calnd_dd":"2026-01-01","dy_tp_cd":"THU","calnd_dd_dy":"2026-01-01","kr_dy_tp":"목요일","holdy_nm":"신정"},
 {"calnd_dd":"2026-06-03","dy_tp_cd":"WED","calnd_dd_dy":"2026-06-03","kr_dy_tp":"수요일","holdy_nm":"임시공휴일"},
 {"calnd_dd":"2026-12-31","dy_tp_cd":"THU","calnd_dd_dy":"2026-12-31","kr_dy_tp":"목요일","holdy_nm":"연말휴장일"}
]}"#;

    fn h(d: &str, w: &str, n: &str) -> Holiday {
        Holiday {
            calnd_dd: d.into(),
            kr_dy_tp: w.into(),
            holdy_nm: n.into(),
        }
    }

    fn sample() -> Vec<Holiday> {
        vec![
            h("2026-01-01", "목요일", "신정"),
            h("2026-06-03", "수요일", "임시공휴일"),
            h("2026-12-31", "목요일", "연말휴장일"),
        ]
    }

    fn dt(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).expect(s)
    }

    /// 테스트마다 따로 쓰는 빈 임시 폴더 (끝나면 지움)
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let p = env::temp_dir().join(format!("krx_test_{}_{}", std::process::id(), name));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn files(&self) -> Vec<String> {
            let mut v: Vec<String> = fs::read_dir(&self.0)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn good_cache_json(year: i32, fetched_at: &str, holidays: &[Holiday]) -> String {
        serde_json::json!({
            "year": year,
            "fetched_at": fetched_at,
            "source": HOLIDAY_URL,
            "holidays": holidays,
        })
        .to_string()
    }

    fn write_cache(dir: &Path, year: i32, text: &str) -> PathBuf {
        let p = cache_path(dir, year);
        fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn parse_sample_keeps_three_fields() {
        assert_eq!(parse_holidays(SAMPLE_RESPONSE, 2026).unwrap(), sample());
    }

    #[test]
    fn parse_bad_responses_error() {
        let cases = [
            ("빈 목록", r#"{"block1":[]}"#),
            ("block1 없음", "{}"),
            (
                "다른 해 날짜",
                r#"{"block1":[{"calnd_dd":"2025-12-31","kr_dy_tp":"수요일","holdy_nm":"x"}]}"#,
            ),
            ("날짜 없음", r#"{"block1":[{"kr_dy_tp":"수요일","holdy_nm":"x"}]}"#),
            ("JSON 아님", "<html>error</html>"),
            ("객체 아님", "[1,2]"),
        ];
        for (name, raw) in cases {
            assert!(parse_holidays(raw, 2026).is_err(), "{name}");
        }
    }

    // (연도, 받은 시각, 지금, 다시 받아야 하나) — test_krx_schedule.py 의 REFRESH_CASES 와 같다
    const REFRESH_CASES: [(i32, &str, &str, bool); 9] = [
        (2026, "2026-11-22T10:00:00+09:00", "2026-11-22T15:30:00+00:00", true),
        (2026, "2026-11-22T10:00:00+09:00", "2026-11-22T14:59:00+00:00", false),
        (2026, "2026-11-23T09:00:00+09:00", "2026-11-25T18:00:00+09:00", false),
        (2026, "2026-11-25T23:59:59+09:00", "2026-11-26T00:00:00+09:00", true),
        (2026, "2026-11-26T00:00:00+09:00", "2026-11-26T12:00:00+09:00", false),
        (2026, "2026-11-23T09:00:00+09:00", "2026-11-29T12:00:00+09:00", true),
        (2027, "2026-11-23T09:00:00+09:00", "2026-11-27T09:00:00+09:00", true),
        (2026, "2026-03-01T09:00:00+09:00", "2027-01-05T09:00:00+09:00", true),
        (2026, "2027-01-01T00:00:00+09:00", "2027-03-11T09:00:00+09:00", false),
    ];

    #[test]
    fn refresh_rule_cases() {
        for (year, fetched, now, expected) in REFRESH_CASES {
            assert_eq!(
                needs_refresh(year, dt(fetched), dt(now)),
                expected,
                "year={year} fetched={fetched} now={now}"
            );
        }
    }

    #[test]
    fn fetched_at_format() {
        assert!(parse_fetched_at("2026-11-22T21:43:32+09:00").is_some());
        for bad in [
            "2026-11-22 21:43:32",
            "2026-11-22T21:43:32",
            "2026-11-22T21:43:32Z",
            "2026-11-22T21:43:32.5+09:00",
            "",
        ] {
            assert!(parse_fetched_at(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn cache_missing_is_none() {
        let t = TempDir::new("missing");
        assert!(load_cache(&t.0, 2026).unwrap().is_none());
    }

    #[test]
    fn cache_good_loads() {
        let t = TempDir::new("good");
        write_cache(
            &t.0,
            2026,
            &good_cache_json(2026, "2026-11-23T09:00:00+09:00", &sample()),
        );
        assert_eq!(load_cache(&t.0, 2026).unwrap().unwrap().holidays, sample());
    }

    #[test]
    fn cache_bad_errors() {
        let t = TempDir::new("bad");
        let ok_at = "2026-11-23T09:00:00+09:00";
        let cases = [
            ("깨진 파일", "garbage".to_string()),
            ("빈 목록", good_cache_json(2026, ok_at, &[])),
            ("연도 다름", good_cache_json(2025, ok_at, &sample())),
            (
                "받은 시각 형식 틀림",
                good_cache_json(2026, "2026-11-23 09:00:00", &sample()),
            ),
            (
                "받은 시각 시간대 없음",
                good_cache_json(2026, "2026-11-23T09:00:00", &sample()),
            ),
            (
                "필드 빠짐",
                serde_json::json!({"year": 2026, "fetched_at": ok_at,
                    "holidays": [{"calnd_dd": "2026-01-01"}]})
                .to_string(),
            ),
        ];
        for (name, text) in cases {
            write_cache(&t.0, 2026, &text);
            assert!(load_cache(&t.0, 2026).is_err(), "{name}");
        }
    }

    #[test]
    fn save_then_load_and_no_tmp_left() {
        let t = TempDir::new("save");
        save_holidays(&t.0, 2026, sample(), dt("2026-11-23T00:00:00+00:00")).unwrap();
        let cache = load_cache(&t.0, 2026).unwrap().unwrap();
        assert_eq!(cache.fetched_at, "2026-11-23T09:00:00+09:00");
        assert_eq!(cache.holidays, sample());
        assert_eq!(t.files(), vec!["krx_holidays_2026.json"]);
    }

    #[test]
    fn save_refuses_empty() {
        let t = TempDir::new("save_empty");
        assert!(save_holidays(&t.0, 2026, vec![], now_kst()).is_err());
        assert!(t.files().is_empty());
    }

    // 수요일. 마지막 갱신 기준 = 11-23 월 0시
    const NOW: &str = "2026-11-25T18:00:00+09:00";

    #[test]
    fn get_miss_fetches_and_saves() {
        let t = TempDir::new("get_miss");
        let calls = Cell::new(0);
        let (cache, note) = get_holidays(&t.0, 2026, dt(NOW), &mut |_| {
            calls.set(calls.get() + 1);
            Ok(sample())
        })
        .unwrap();
        assert_eq!(
            (calls.get(), note, cache.fetched_at.as_str()),
            (1, NOTE_NEW, NOW)
        );
        assert!(load_cache(&t.0, 2026).unwrap().is_some());
    }

    #[test]
    fn get_fresh_hit_does_not_fetch() {
        let t = TempDir::new("get_hit");
        write_cache(
            &t.0,
            2026,
            &good_cache_json(2026, "2026-11-23T09:00:00+09:00", &sample()),
        );
        let calls = Cell::new(0);
        let (_, note) = get_holidays(&t.0, 2026, dt(NOW), &mut |_| {
            calls.set(calls.get() + 1);
            Err("호출되면 안 됨".into())
        })
        .unwrap();
        assert_eq!((calls.get(), note), (0, NOTE_CACHED));
    }

    #[test]
    fn get_stale_refetches_and_saves() {
        let t = TempDir::new("get_stale");
        write_cache(
            &t.0,
            2026,
            &good_cache_json(2026, "2026-11-22T09:00:00+09:00", &sample()),
        );
        let calls = Cell::new(0);
        let (cache, note) = get_holidays(&t.0, 2026, dt(NOW), &mut |_| {
            calls.set(calls.get() + 1);
            Ok(sample()[..2].to_vec())
        })
        .unwrap();
        assert_eq!(
            (calls.get(), note, cache.holidays.len()),
            (1, NOTE_REFRESHED, 2)
        );
        assert_eq!(
            load_cache(&t.0, 2026).unwrap().unwrap().fetched_at,
            cache.fetched_at
        );
    }

    #[test]
    fn get_stale_with_failing_fetch_errors_and_keeps_file() {
        let t = TempDir::new("get_stale_fail");
        let p = write_cache(
            &t.0,
            2026,
            &good_cache_json(2026, "2026-11-22T09:00:00+09:00", &sample()),
        );
        let before = fs::read(&p).unwrap();
        assert!(get_holidays(&t.0, 2026, dt(NOW), &mut |_| Err("KRX 다운".into())).is_err());
        assert_eq!(fs::read(&p).unwrap(), before);
    }

    #[test]
    fn get_future_year_is_not_saved() {
        let t = TempDir::new("get_future");
        let calls = Cell::new(0);
        let (_, note) = get_holidays(&t.0, 2027, dt(NOW), &mut |_| {
            calls.set(calls.get() + 1);
            Ok(vec![h("2027-01-01", "금요일", "신정")])
        })
        .unwrap();
        assert_eq!((calls.get(), note), (1, NOTE_PREVIEW));
        assert!(t.files().is_empty());
    }

    // ---- CI에서 Python과 저장 파일을 주고받는 확인용 (cargo test -- --ignored) ----

    #[test]
    #[ignore]
    fn roundtrip_read_from_python() {
        let dir = PathBuf::from(env::var("KRX_ROUNDTRIP_READ").expect("KRX_ROUNDTRIP_READ"));
        let cache = load_cache(&dir, 2026)
            .unwrap()
            .expect("Python 저장본이 없음");
        assert_eq!(cache.holidays, sample());
        assert_eq!(cache.fetched_at, "2026-11-23T09:00:00+09:00");
    }

    #[test]
    #[ignore]
    fn roundtrip_write_for_python() {
        let dir = PathBuf::from(env::var("KRX_ROUNDTRIP_WRITE").expect("KRX_ROUNDTRIP_WRITE"));
        save_holidays(&dir, 2026, sample(), dt("2026-11-23T09:00:00+09:00")).unwrap();
    }
}
