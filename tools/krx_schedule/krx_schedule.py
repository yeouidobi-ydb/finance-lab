#!/usr/bin/env python3
"""
KRX 휴장일 및 장운영 시간 변경(수능일 등) 조회 스크립트 (표준 라이브러리 전용)
- 휴장일: open.krx.co.kr GenerateOTP + OPN99000001.jspx
- 시간변경/임시공휴일 공지: strn.krx.co.kr/corebbs5/BHPSTRN0401/list
"""

from __future__ import annotations

import json
import re
import sys
import urllib.parse
import urllib.request
from datetime import date, datetime

UA = "Mozilla/5.0"
OTP_URL = "https://open.krx.co.kr/contents/COM/GenerateOTP.jspx?name=form&bld=MKD/01/0110/01100305/mkd01100305_01"
HOLIDAY_URL = "https://open.krx.co.kr/contents/OPN/99/OPN99000001.jspx"
STRN_LIST_URL = "https://strn.krx.co.kr/corebbs5/BHPSTRN0401/list"

KEYWORDS = ("수능", "임시", "휴장", "시간", "개장", "대체", "선거", "변경")


def fetch_holidays(year: int) -> list[dict[str, str]]:
    otp_req = urllib.request.Request(OTP_URL, headers={"User-Agent": UA})
    with urllib.request.urlopen(otp_req, timeout=10) as resp:
        code = resp.read().decode("utf-8").strip()

    body = urllib.parse.urlencode(
        {
            "search_bas_yy": str(year),
            "gridTp": "KRX",
            "pagePath": "/contents/MKD/01/0110/01100305/MKD01100305.jsp",
            "code": code,
            "pageFirstCall": "Y",
        }
    ).encode("utf-8")

    req = urllib.request.Request(
        HOLIDAY_URL,
        data=body,
        headers={
            "User-Agent": UA,
            "Content-Type": "application/x-www-form-urlencoded; charset=UTF-8",
        },
    )
    with urllib.request.urlopen(req, timeout=10) as resp:
        data = json.loads(resp.read().decode("utf-8"))

    return data.get("block1", [])


def fetch_notices(pages: int = 1) -> list[dict[str, str]]:
    results: list[dict[str, str]] = []
    seen: set[str] = set()
    pattern = re.compile(
        r'href="\./view\?bbsSeq=(\d+)"[^>]*>(.*?)</a>.*?(\d{4}-\d{2}-\d{2})',
        re.S,
    )

    for page in range(1, pages + 1):
        url = f"{STRN_LIST_URL}?curPage={page}"
        req = urllib.request.Request(url, headers={"User-Agent": UA})
        with urllib.request.urlopen(req, timeout=10) as resp:
            html = resp.read().decode("utf-8", errors="ignore")

        for bbs_seq, raw_title, posted_at in pattern.findall(html):
            if bbs_seq in seen:
                continue
            seen.add(bbs_seq)
            title = re.sub(r"\s+", " ", raw_title).strip()
            if any(k in title for k in KEYWORDS):
                results.append(
                    {
                        "bbs_seq": bbs_seq,
                        "posted_at": posted_at,
                        "title": title,
                        "url": f"https://strn.krx.co.kr/corebbs5/BHPSTRN0401/view?bbsSeq={bbs_seq}",
                    }
                )
    return results


def check_date(target: date) -> dict[str, object]:
    holidays = fetch_holidays(target.year)
    target_str = target.strftime("%Y-%m-%d")
    holiday_match = next((h for h in holidays if h.get("calnd_dd") == target_str), None)

    notices = fetch_notices(pages=1)
    # 공지 제목에 YYYY.MM.DD / YYYY-MM-DD / YYYY년 M월 D일 형태로 날짜가 포함된 경우 매칭
    date_tokens = {
        target_str,
        target.strftime("%Y.%m.%d"),
        f"{target.year}년 {target.month}월 {target.day}일",
        f"{target.year}년 {target.month:02d}월 {target.day:02d}일",
    }
    notice_matches = [n for n in notices if any(tok in n["title"] for tok in date_tokens)]

    return {
        "date": target_str,
        "is_weekend": target.weekday() >= 5,
        "holiday": holiday_match,
        "notices": notice_matches,
    }


def main() -> None:
    target = (
        datetime.strptime(sys.argv[1], "%Y-%m-%d").date()
        if len(sys.argv) > 1
        else date.today()
    )

    status = check_date(target)
    print(f"=== {status['date']} KRX 장운영 체크 ===")
    print(f"- 주말 여부: {status['is_weekend']}")
    print(f"- 공식 휴장일: {status['holiday'] or '없음'}")
    print(f"- 관련 공지({len(status['notices'])}건):")
    for n in status["notices"]:
        print(f"  * [{n['posted_at']}] {n['title']} ({n['url']})")

    print(f"\n=== {target.year}년 전체 휴장일 ===")
    for h in fetch_holidays(target.year):
        print(f"  {h['calnd_dd']} ({h['kr_dy_tp']}) - {h['holdy_nm']}")


if __name__ == "__main__":
    main()
