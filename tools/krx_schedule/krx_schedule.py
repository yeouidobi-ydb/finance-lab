#!/usr/bin/env python3
"""
KRX 휴장일 및 장운영 관련 공지 조회 스크립트 (표준 라이브러리 전용)
- 휴장일: open.krx.co.kr GenerateOTP + OPN99000001.jspx
- 참고 공지: strn.krx.co.kr/corebbs5/BHPSTRN0401/list
  주의: 이 게시판은 'KRX 개인 공매도 모의거래인증시스템' 게시판이다.
  실제 정규장 시간 변경 공지가 아니므로 참고용으로만 표시한다.

사용법: python krx_schedule.py [YYYY-MM-DD]   (날짜 생략 시 오늘, KST 기준)
"""

from __future__ import annotations

import json
import re
import sys
import urllib.parse
import urllib.request
from datetime import date, datetime, timedelta, timezone

UA = "Mozilla/5.0"
OTP_URL = "https://open.krx.co.kr/contents/COM/GenerateOTP.jspx?name=form&bld=MKD/01/0110/01100305/mkd01100305_01"
HOLIDAY_URL = "https://open.krx.co.kr/contents/OPN/99/OPN99000001.jspx"
STRN_LIST_URL = "https://strn.krx.co.kr/corebbs5/BHPSTRN0401/list"

KST = timezone(timedelta(hours=9))
KEYWORDS = ("수능", "임시", "휴장", "시간", "개장", "대체", "선거", "변경")


class KrxError(RuntimeError):
    """KRX 응답을 믿을 수 없을 때 발생. '휴장일 없음'으로 넘기지 않는다."""


def fetch_holidays(year: int) -> list[dict[str, str]]:
    otp_req = urllib.request.Request(OTP_URL, headers={"User-Agent": UA})
    with urllib.request.urlopen(otp_req, timeout=10) as resp:
        code = resp.read().decode("utf-8").strip()
    if not code:
        raise KrxError("OTP 발급 실패: 빈 응답")

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
        raw = resp.read().decode("utf-8")

    try:
        data = json.loads(raw)
    except json.JSONDecodeError as e:
        raise KrxError(f"휴장일 응답이 JSON이 아님: {raw[:200]!r}") from e

    holidays = data.get("block1")
    # 매년 최소 신정·연말휴장일이 있으므로 비어 있으면 조회 실패로 본다.
    if not holidays:
        raise KrxError(f"{year}년 휴장일 목록이 비어 있음 (조회 실패 가능성)")
    return holidays


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
                        "posted_at": posted_at,
                        "title": title,
                        "url": f"https://strn.krx.co.kr/corebbs5/BHPSTRN0401/view?bbsSeq={bbs_seq}",
                    }
                )
    return results


def date_tokens(target: date) -> set[str]:
    return {
        target.strftime("%Y-%m-%d"),
        target.strftime("%Y.%m.%d"),
        f"{target.year}년 {target.month}월 {target.day}일",
        f"{target.year}년 {target.month:02d}월 {target.day:02d}일",
    }


def check_date(target: date, holidays: list[dict[str, str]], notices: list[dict[str, str]]) -> dict[str, object]:
    target_str = target.strftime("%Y-%m-%d")
    holiday = next((h for h in holidays if h.get("calnd_dd") == target_str), None)
    is_weekend = target.weekday() >= 5
    tokens = date_tokens(target)
    notice_matches = [n for n in notices if any(t in n["title"] for t in tokens)]

    if is_weekend:
        verdict = "휴장 (주말)"
    elif holiday:
        verdict = f"휴장 ({holiday.get('holdy_nm', '')})"
    else:
        verdict = "개장"

    return {
        "date": target_str,
        "is_weekend": is_weekend,
        "holiday": holiday,
        "is_trading_day": not is_weekend and holiday is None,
        "verdict": verdict,
        "notices": notice_matches,
    }


def is_trading_day(target: date) -> bool:
    """다른 스크립트에서 import해서 쓰는 용도."""
    if target.weekday() >= 5:
        return False
    target_str = target.strftime("%Y-%m-%d")
    return all(h.get("calnd_dd") != target_str for h in fetch_holidays(target.year))


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    target = (
        datetime.strptime(sys.argv[1], "%Y-%m-%d").date()
        if len(sys.argv) > 1
        else datetime.now(KST).date()
    )

    try:
        holidays = fetch_holidays(target.year)
        notices = fetch_notices(pages=1)
    except (KrxError, OSError) as e:
        print(f"[오류] KRX 조회 실패: {e}", file=sys.stderr)
        return 1

    status = check_date(target, holidays, notices)
    h = status["holiday"]
    print(f"=== {status['date']} KRX 장운영 체크 ===")
    print(f"- 판정: {status['verdict']}")
    print(f"- 주말 여부: {status['is_weekend']}")
    print(f"- 공식 휴장일: {h['holdy_nm'] + ' (' + h['kr_dy_tp'] + ')' if h else '없음'}")
    print(f"- 참고 공지 · 공매도 모의거래 게시판 ({len(status['notices'])}건):")
    for n in status["notices"]:
        print(f"  * [{n['posted_at']}] {n['title']} ({n['url']})")

    print(f"\n=== {target.year}년 전체 휴장일 ===")
    for h in holidays:
        print(f"  {h.get('calnd_dd', '')} ({h.get('kr_dy_tp', '')}) - {h.get('holdy_nm', '')}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
