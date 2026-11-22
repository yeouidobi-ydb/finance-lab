#!/usr/bin/env python3
"""
KRX 휴장일 및 장운영 관련 공지 조회 스크립트 (표준 라이브러리 전용)
- 휴장일: open.krx.co.kr GenerateOTP + OPN99000001.jspx
  연도별로 한 번 받아 data/krx_holidays_<연도>.json 에 저장하고, 이후에는 저장본을 쓴다.
  (Rust 버전과 같은 파일을 같이 쓴다. 위치는 KRX_SCHEDULE_DATA_DIR 로 바꿀 수 있다.)
- 참고 공지: strn.krx.co.kr/corebbs5/BHPSTRN0401/list
  주의: 이 게시판은 'KRX 개인 공매도 모의거래인증시스템' 게시판이다.
  실제 정규장 시간 변경 공지가 아니므로 참고용으로만 표시한다.

사용법:
  python krx_schedule.py [YYYY-MM-DD]   날짜 조회 (생략 시 오늘, KST). 그 해 저장본이 없으면 받아서 저장
  python krx_schedule.py --update [YYYY] 그 해 휴장일을 다시 받아 저장본 교체 (생략 시 올해, KST)
"""

from __future__ import annotations

import json
import os
import re
import sys
import urllib.parse
import urllib.request
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

UA = "Mozilla/5.0"
OTP_URL = "https://open.krx.co.kr/contents/COM/GenerateOTP.jspx?name=form&bld=MKD/01/0110/01100305/mkd01100305_01"
HOLIDAY_URL = "https://open.krx.co.kr/contents/OPN/99/OPN99000001.jspx"
STRN_LIST_URL = "https://strn.krx.co.kr/corebbs5/BHPSTRN0401/list"

KST = timezone(timedelta(hours=9))
KEYWORDS = ("수능", "임시", "휴장", "시간", "개장", "대체", "선거", "변경")
DATA_DIR_ENV = "KRX_SCHEDULE_DATA_DIR"


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
    # 저장본을 Rust와 같이 쓰므로 세 필드만 문자열로 남긴다.
    normalized = [
        {
            "calnd_dd": str(h.get("calnd_dd") or ""),
            "kr_dy_tp": str(h.get("kr_dy_tp") or ""),
            "holdy_nm": str(h.get("holdy_nm") or ""),
        }
        for h in holidays
        if isinstance(h, dict)
    ]
    # 날짜가 비었거나 다른 해 날짜가 섞이면 응답을 믿지 않는다 (기존 저장본도 덮어쓰지 않음).
    bad = [h for h in normalized if not valid_holiday_date(h["calnd_dd"], year)]
    if bad or len(normalized) != len(holidays):
        raise KrxError(f"{year}년 휴장일 응답에 이상한 항목이 있음: {(bad or holidays)[:3]!r}")
    return normalized


def valid_holiday_date(value: str, year: int) -> bool:
    try:
        return datetime.strptime(value, "%Y-%m-%d").year == year and len(value) == 10
    except ValueError:
        return False


def data_dir() -> Path:
    override = os.environ.get(DATA_DIR_ENV)
    return Path(override) if override else Path(__file__).resolve().parent / "data"


def cache_path(year: int) -> Path:
    return data_dir() / f"krx_holidays_{year}.json"


def save_holidays(year: int, holidays: list[dict[str, str]]) -> dict[str, object]:
    if not holidays:
        raise KrxError(f"{year}년 휴장일 목록이 비어 있어 저장하지 않음")
    cache = {
        "year": year,
        "fetched_at": datetime.now(KST).isoformat(timespec="seconds"),
        "source": HOLIDAY_URL,
        "holidays": holidays,
    }
    path = cache_path(year)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(cache, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    os.replace(tmp, path)  # 쓰다 끊겨도 기존 저장본이 깨지지 않게
    return cache


def load_cache(year: int) -> dict[str, object] | None:
    """저장본이 없으면 None. 있는데 이상하면 오류로 멈춘다."""
    path = cache_path(year)
    if not path.exists():
        return None
    try:
        cache = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as e:
        raise KrxError(f"휴장일 저장본을 읽을 수 없음 ({path}): {e} — --update {year} 로 다시 받으세요") from e

    holidays = cache.get("holidays") if isinstance(cache, dict) else None
    if (
        not isinstance(cache, dict)
        or cache.get("year") != year
        or not isinstance(cache.get("fetched_at"), str)
        or not isinstance(holidays, list)
        or not holidays
        or not all(
            isinstance(h, dict)
            and all(isinstance(h.get(k), str) for k in ("calnd_dd", "kr_dy_tp", "holdy_nm"))
            and valid_holiday_date(h["calnd_dd"], year)
            for h in holidays
        )
    ):
        raise KrxError(f"휴장일 저장본 형식이 이상함 ({path}) — --update {year} 로 다시 받으세요")
    return cache


def get_holidays(year: int) -> tuple[list[dict[str, str]], str, str]:
    """(휴장일 목록, 받은 시각, 출처 설명). 저장본이 없을 때만 KRX에 묻는다.

    올해(KST)보다 뒤의 연도는 KRX가 확정 전 달력(연말휴장일 등이 빠짐)을 주므로
    저장하지 않고 그때그때 받아서 쓴다. 저장하려면 --update <연도>를 직접 돌린다.
    """
    cache = load_cache(year)
    if cache is not None:
        return cache["holidays"], cache["fetched_at"], "저장본 사용"  # type: ignore[return-value]
    holidays = fetch_holidays(year)
    if year > datetime.now(KST).year:
        fetched_at = datetime.now(KST).isoformat(timespec="seconds")
        return holidays, fetched_at, "다음 해 이후라 저장 안 함 · 확정 전일 수 있음"
    cache = save_holidays(year, holidays)
    return cache["holidays"], cache["fetched_at"], "방금 새로 받아 저장"  # type: ignore[return-value]


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
    holidays, _, _ = get_holidays(target.year)
    return all(h.get("calnd_dd") != target_str for h in holidays)


USAGE = "사용법: krx_schedule [YYYY-MM-DD] | krx_schedule --update [YYYY]"


def run_update(args: list[str]) -> int:
    if len(args) > 1 or (args and not re.fullmatch(r"\d{4}", args[0])):
        print(f"[오류] 연도는 YYYY 형식 하나만 받음\n{USAGE}", file=sys.stderr)
        return 2
    year = int(args[0]) if args else datetime.now(KST).year
    try:
        cache = save_holidays(year, fetch_holidays(year))
    except (KrxError, OSError) as e:
        print(f"[오류] KRX 조회 실패: {e}", file=sys.stderr)
        return 1
    holidays = cache["holidays"]
    print(f"=== {year}년 휴장일 저장 완료 ({len(holidays)}일, {cache['fetched_at']} 받음) ===")  # type: ignore[arg-type]
    print(f"- 저장 위치: {cache_path(year)}")
    for h in holidays:  # type: ignore[union-attr]
        print(f"  {h['calnd_dd']} ({h['kr_dy_tp']}) - {h['holdy_nm']}")
    return 0


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(encoding="utf-8")

    args = sys.argv[1:]
    if args and args[0] == "--update":
        return run_update(args[1:])
    if len(args) > 1:
        print(USAGE, file=sys.stderr)
        return 2

    try:
        target = datetime.strptime(args[0], "%Y-%m-%d").date() if args else datetime.now(KST).date()
    except ValueError:
        print(f"[오류] 날짜는 YYYY-MM-DD 형식\n{USAGE}", file=sys.stderr)
        return 2

    try:
        holidays, fetched_at, data_note = get_holidays(target.year)
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
    print(f"- 휴장일 데이터: {fetched_at[:10]} 받음 ({data_note})")
    print(f"- 참고 공지 · 공매도 모의거래 게시판 ({len(status['notices'])}건):")
    for n in status["notices"]:
        print(f"  * [{n['posted_at']}] {n['title']} ({n['url']})")

    print(f"\n=== {target.year}년 전체 휴장일 ===")
    for h in holidays:
        print(f"  {h.get('calnd_dd', '')} ({h.get('kr_dy_tp', '')}) - {h.get('holdy_nm', '')}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
