use chrono::{Datelike, Local, NaiveDate};
use regex::Regex;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::collections::HashSet;
use std::env;
use std::error::Error;

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
    kr_dy_tp: String,
    holdy_nm: String,
}

#[derive(Debug, Clone)]
struct Notice {
    bbs_seq: String,
    posted_at: String,
    title: String,
    url: String,
}

fn fetch_holidays(client: &Client, year: i32) -> Result<Vec<Holiday>, Box<dyn Error>> {
    let code = client
        .get(OTP_URL)
        .header("User-Agent", UA)
        .send()?
        .text()?
        .trim()
        .to_string();

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

    let resp: HolidayResponse = client
        .post(HOLIDAY_URL)
        .header("User-Agent", UA)
        .form(&params)
        .send()?
        .json()?;

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
        let html = client.get(&url).header("User-Agent", UA).send()?.text()?;

        for cap in re.captures_iter(&html) {
            let bbs_seq = cap[1].to_string();
            if !seen.insert(bbs_seq.clone()) {
                continue;
            }
            let title = ws_re.replace_all(&cap[2], " ").trim().to_string();
            let posted_at = cap[3].to_string();

            if KEYWORDS.iter().any(|k| title.contains(k)) {
                results.push(Notice {
                    url: format!(
                        "https://strn.krx.co.kr/corebbs5/BHPSTRN0401/view?bbsSeq={bbs_seq}"
                    ),
                    bbs_seq,
                    posted_at,
                    title,
                });
            }
        }
    }

    Ok(results)
}

fn main() -> Result<(), Box<dyn Error>> {
    let target = match env::args().nth(1) {
        Some(arg) => NaiveDate::parse_from_str(&arg, "%Y-%m-%d")?,
        None => Local::now().date_naive(),
    };

    let client = Client::builder().build()?;
    let holidays = fetch_holidays(&client, target.year())?;
    let notices = fetch_notices(&client, 1)?;

    let target_str = target.format("%Y-%m-%d").to_string();
    let is_weekend = target.weekday().num_days_from_monday() >= 5;
    let holiday_match = holidays.iter().find(|h| h.calnd_dd == target_str);

    let tokens = [
        target_str.clone(),
        target.format("%Y.%m.%d").to_string(),
        format!("{}년 {}월 {}일", target.year(), target.month(), target.day()),
        format!(
            "{}년 {:02}월 {:02}일",
            target.year(),
            target.month(),
            target.day()
        ),
    ];

    let notice_matches: Vec<&Notice> = notices
        .iter()
        .filter(|n| tokens.iter().any(|t| n.title.contains(t)))
        .collect();

    println!("=== {} KRX 장운영 체크 ===", target_str);
    println!("- 주말 여부: {}", is_weekend);
    match holiday_match {
        Some(h) => println!("- 공식 휴장일: {} ({})", h.holdy_nm, h.kr_dy_tp),
        None => println!("- 공식 휴장일: 없음"),
    }
    println!("- 관련 공지({}건):", notice_matches.len());
    for n in notice_matches {
        println!("  * [{}] {} ({})", n.posted_at, n.title, n.url);
    }

    println!("\n=== {}년 전체 휴장일 ===", target.year());
    for h in &holidays {
        println!("  {} ({}) - {}", h.calnd_dd, h.kr_dy_tp, h.holdy_nm);
    }

    Ok(())
}
