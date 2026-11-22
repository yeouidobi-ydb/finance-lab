//! KRX 휴장일 및 장운영 관련 공지 조회 (krx_schedule.py와 같은 동작)
//! - 휴장일: open.krx.co.kr GenerateOTP + OPN99000001.jspx
//! - 참고 공지: strn.krx.co.kr/corebbs5/BHPSTRN0401/list
//!   주의: 'KRX 개인 공매도 모의거래인증시스템' 게시판이다. 정규장 공지가 아니므로 참고용.
//!
//! 사용법: krx_schedule [YYYY-MM-DD]   (날짜 생략 시 오늘, KST 기준)

use chrono::{Datelike, FixedOffset, NaiveDate, Utc};
use regex::Regex;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::collections::HashSet;
use std::env;
use std::error::Error;
use std::process::ExitCode;
use std::time::Duration;

const UA: &str = "Mozilla/5.0";
const OTP_URL: &str =
    "https://open.krx.co.kr/contents/COM/GenerateOTP.jspx?name=form&bld=MKD/01/0110/01100305/mkd01100305_01";
const HOLIDAY_URL: &str = "https://open.krx.co.kr/contents/OPN/99/OPN99000001.jspx";
const STRN_LIST_URL: &str = "https://strn.krx.co.kr/corebbs5/BHPSTRN0401/list";
const KEYWORDS: [&str; 8] = ["수능", "임시", "휴장", "시간", "개장", "대체", "선거", "변경"];

#[derive(Debug, Deserialize)]
struct HolidayResponse {
    #[serde(default)]
    block1: Vec<Holiday>,
}

#[derive(Debug, Deserialize, Clone)]
struct Holiday {
    calnd_dd: String,
    #[serde(default)]
    kr_dy_tp: Option<String>,
    #[serde(default)]
    holdy_nm: Option<String>,
}

impl Holiday {
    fn day_name(&self) -> &str {
        self.kr_dy_tp.as_deref().unwrap_or("")
    }
    fn name(&self) -> &str {
        self.holdy_nm.as_deref().unwrap_or("")
    }
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
    Ok(resp.block1)
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

fn today_kst() -> NaiveDate {
    let kst = FixedOffset::east_opt(9 * 3600).expect("valid offset");
    Utc::now().with_timezone(&kst).date_naive()
}

fn run() -> Result<(), Box<dyn Error>> {
    let target = match env::args().nth(1) {
        Some(arg) => NaiveDate::parse_from_str(&arg, "%Y-%m-%d")?,
        None => today_kst(),
    };

    let client = Client::builder().timeout(Duration::from_secs(10)).build()?;
    let holidays = fetch_holidays(&client, target.year())?;
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
        "- 참고 공지 · 공매도 모의거래 게시판 ({}건):",
        notice_matches.len()
    );
    for n in notice_matches {
        println!("  * [{}] {} ({})", n.posted_at, n.title, n.url);
    }

    println!("\n=== {}년 전체 휴장일 ===", target.year());
    for h in &holidays {
        println!("  {} ({}) - {}", h.calnd_dd, h.day_name(), h.name());
    }

    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[오류] KRX 조회 실패: {e}");
            ExitCode::FAILURE
        }
    }
}
