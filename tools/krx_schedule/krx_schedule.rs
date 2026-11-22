//! KRX 휴장일 및 장운영 관련 공지 조회 (krx_schedule.py와 같은 동작)
//! - 휴장일: open.krx.co.kr GenerateOTP + OPN99000001.jspx
//!   연도별로 한 번 받아 data/krx_holidays_<연도>.json 에 저장하고, 이후에는 저장본을 쓴다.
//!   (Python 버전과 같은 파일을 같이 쓴다. 위치는 KRX_SCHEDULE_DATA_DIR 로 바꿀 수 있다.)
//! - 참고 공지: strn.krx.co.kr/corebbs5/BHPSTRN0401/list
//!   주의: 'KRX 개인 공매도 모의거래인증시스템' 게시판이다. 정규장 공지가 아니므로 참고용.
//!
//! 사용법:
//!   krx_schedule [YYYY-MM-DD]    날짜 조회 (생략 시 오늘, KST). 그 해 저장본이 없으면 받아서 저장
//!   krx_schedule --update [YYYY] 그 해 휴장일을 다시 받아 저장본 교체 (생략 시 올해, KST)

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Utc};
use regex::Regex;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
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

#[derive(Debug, Deserialize)]
struct HolidayResponse {
    #[serde(default)]
    block1: Vec<RawHoliday>,
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
#[derive(Debug, Serialize, Deserialize, Clone)]
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

fn fetch_holidays(client: &Client, year: i32) -> Result<Vec<Holiday>, Box<dyn Error>> {
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

    let resp: HolidayResponse = serde_json::from_str(&raw).map_err(|e| {
        let head: String = raw.chars().take(200).collect();
        format!("휴장일 응답이 JSON이 아님 ({e}): {head:?}")
    })?;

    // 매년 최소 신정·연말휴장일이 있으므로 비어 있으면 조회 실패로 본다.
    if resp.block1.is_empty() {
        return Err(format!("{year}년 휴장일 목록이 비어 있음 (조회 실패 가능성)").into());
    }
    // 저장본을 Python과 같이 쓰므로 세 필드만 문자열로 남긴다.
    let holidays: Vec<Holiday> = resp
        .block1
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

fn data_dir() -> PathBuf {
    match env::var_os(DATA_DIR_ENV) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        // 실행 파일 위치는 빌드 방식마다 달라서, 소스 폴더(= Python 파일 옆) 기준으로 둔다.
        _ => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data"),
    }
}

fn cache_path(year: i32) -> PathBuf {
    data_dir().join(format!("krx_holidays_{year}.json"))
}

fn now_kst() -> DateTime<FixedOffset> {
    let kst = FixedOffset::east_opt(9 * 3600).expect("valid offset");
    Utc::now().with_timezone(&kst)
}

fn today_kst() -> NaiveDate {
    now_kst().date_naive()
}

fn new_cache(year: i32, holidays: Vec<Holiday>) -> HolidayCache {
    HolidayCache {
        year,
        fetched_at: now_kst().format("%Y-%m-%dT%H:%M:%S%:z").to_string(),
        source: HOLIDAY_URL.to_string(),
        holidays,
    }
}

fn save_holidays(year: i32, holidays: Vec<Holiday>) -> Result<HolidayCache, Box<dyn Error>> {
    if holidays.is_empty() {
        return Err(format!("{year}년 휴장일 목록이 비어 있어 저장하지 않음").into());
    }
    let cache = new_cache(year, holidays);
    let path = cache_path(year);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!("krx_holidays_{year}.json.tmp"));
    fs::write(&tmp, serde_json::to_string_pretty(&cache)? + "\n")?;
    fs::rename(&tmp, &path)?; // 쓰다 끊겨도 기존 저장본이 깨지지 않게
    Ok(cache)
}

/// 저장본이 없으면 None. 있는데 이상하면 오류로 멈춘다.
fn load_cache(year: i32) -> Result<Option<HolidayCache>, Box<dyn Error>> {
    let path = cache_path(year);
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

/// (휴장일 데이터, 출처 설명). 저장본이 없을 때만 KRX에 묻는다.
///
/// 올해(KST)보다 뒤의 연도는 KRX가 확정 전 달력(연말휴장일 등이 빠짐)을 주므로
/// 저장하지 않고 그때그때 받아서 쓴다. 저장하려면 --update <연도>를 직접 돌린다.
fn get_holidays(
    client: &Client,
    year: i32,
) -> Result<(HolidayCache, &'static str), Box<dyn Error>> {
    if let Some(cache) = load_cache(year)? {
        return Ok((cache, "저장본 사용"));
    }
    let holidays = fetch_holidays(client, year)?;
    if year > now_kst().year() {
        return Ok((
            new_cache(year, holidays),
            "다음 해 이후라 저장 안 함 · 확정 전일 수 있음",
        ));
    }
    Ok((save_holidays(year, holidays)?, "방금 새로 받아 저장"))
}

fn fetch_notices(client: &Client, pages: usize) -> Result<Vec<Notice>, Box<dyn Error>> {
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

fn build_client() -> Result<Client, Box<dyn Error>> {
    Ok(Client::builder().timeout(Duration::from_secs(10)).build()?)
}

fn run_update(year: i32) -> Result<(), Box<dyn Error>> {
    let client = build_client()?;
    let cache = save_holidays(year, fetch_holidays(&client, year)?)?;
    println!(
        "=== {}년 휴장일 저장 완료 ({}일, {} 받음) ===",
        year,
        cache.holidays.len(),
        cache.fetched_at
    );
    println!("- 저장 위치: {}", cache_path(year).display());
    for h in &cache.holidays {
        println!("  {} ({}) - {}", h.calnd_dd, h.day_name(), h.name());
    }
    Ok(())
}

fn run_check(target: NaiveDate) -> Result<(), Box<dyn Error>> {
    let client = build_client()?;
    let (cache, data_note) = get_holidays(&client, target.year())?;
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
