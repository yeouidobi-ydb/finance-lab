"""krx_schedule 테스트 (표준 라이브러리 unittest, 네트워크 호출 없음)

실행: python -m unittest -v test_krx_schedule      (tools/krx_schedule 폴더에서)
같은 경우들을 krx_schedule.rs 의 #[cfg(test)] 에서도 똑같이 확인한다.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from datetime import datetime
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import krx_schedule as k  # noqa: E402

# 실제 KRX 응답 모양 (저장하지 않는 필드 dy_tp_cd, calnd_dd_dy 포함)
SAMPLE_RESPONSE = """{"block1":[
 {"calnd_dd":"2026-01-01","dy_tp_cd":"THU","calnd_dd_dy":"2026-01-01","kr_dy_tp":"목요일","holdy_nm":"신정"},
 {"calnd_dd":"2026-06-03","dy_tp_cd":"WED","calnd_dd_dy":"2026-06-03","kr_dy_tp":"수요일","holdy_nm":"임시공휴일"},
 {"calnd_dd":"2026-12-31","dy_tp_cd":"THU","calnd_dd_dy":"2026-12-31","kr_dy_tp":"목요일","holdy_nm":"연말휴장일"}
]}"""

SAMPLE_HOLIDAYS = [
    {"calnd_dd": "2026-01-01", "kr_dy_tp": "목요일", "holdy_nm": "신정"},
    {"calnd_dd": "2026-06-03", "kr_dy_tp": "수요일", "holdy_nm": "임시공휴일"},
    {"calnd_dd": "2026-12-31", "kr_dy_tp": "목요일", "holdy_nm": "연말휴장일"},
]

# (연도, 받은 시각, 지금, 다시 받아야 하나) — 2026-11-22 일요일, 23 월, 25 수, 26 목, 29 일
REFRESH_CASES = [
    (2026, "2026-11-22T10:00:00+09:00", "2026-11-22T15:30:00+00:00", True),   # UTC 일요일 = KST 월요일 00:30
    (2026, "2026-11-22T10:00:00+09:00", "2026-11-22T14:59:00+00:00", False),  # KST 일요일 23:59
    (2026, "2026-11-23T09:00:00+09:00", "2026-11-25T18:00:00+09:00", False),  # 월 받고 수
    (2026, "2026-11-25T23:59:59+09:00", "2026-11-26T00:00:00+09:00", True),   # 목 0시 경계
    (2026, "2026-11-26T00:00:00+09:00", "2026-11-26T12:00:00+09:00", False),  # 목 0시에 받음
    (2026, "2026-11-23T09:00:00+09:00", "2026-11-29T12:00:00+09:00", True),   # 월 받고 다음 일요일 (목 지남)
    (2027, "2026-11-23T09:00:00+09:00", "2026-11-27T09:00:00+09:00", True),   # 미리 받은 다음 해도 갱신
    (2026, "2026-03-01T09:00:00+09:00", "2027-01-05T09:00:00+09:00", True),   # 지난해 저장본, 해 끝나기 전에 받음
    (2026, "2027-01-01T00:00:00+09:00", "2027-03-11T09:00:00+09:00", False),  # 지난해 저장본, 해 끝난 뒤 받음 (목요일이어도)
]


def dt(value: str) -> datetime:
    return datetime.fromisoformat(value)


def write_cache(directory: Path, year: int, obj: object) -> Path:
    path = k.cache_path(year, directory)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(obj if isinstance(obj, str) else json.dumps(obj, ensure_ascii=False), encoding="utf-8")
    return path


class FakeFetcher:
    def __init__(self, result: list[dict[str, str]] | Exception):
        self.result = result
        self.calls = 0

    def __call__(self, year: int) -> list[dict[str, str]]:
        self.calls += 1
        if isinstance(self.result, Exception):
            raise self.result
        return self.result


class ParseHolidaysTest(unittest.TestCase):
    def test_sample_response_keeps_three_fields(self):
        self.assertEqual(k.parse_holidays(SAMPLE_RESPONSE, 2026), SAMPLE_HOLIDAYS)

    def test_bad_responses_raise(self):
        cases = {
            "빈 목록": '{"block1":[]}',
            "block1 없음": "{}",
            "다른 해 날짜": '{"block1":[{"calnd_dd":"2025-12-31","kr_dy_tp":"수요일","holdy_nm":"x"}]}',
            "날짜 없음": '{"block1":[{"kr_dy_tp":"수요일","holdy_nm":"x"}]}',
            "JSON 아님": "<html>error</html>",
            "객체 아님": "[1,2]",
        }
        for name, raw in cases.items():
            with self.subTest(name), self.assertRaises(k.KrxError):
                k.parse_holidays(raw, 2026)


class RefreshRuleTest(unittest.TestCase):
    def test_cases(self):
        for year, fetched, now, expected in REFRESH_CASES:
            with self.subTest(year=year, fetched=fetched, now=now):
                self.assertEqual(k.needs_refresh(year, dt(fetched), dt(now)), expected)

    def test_fetched_at_format(self):
        self.assertIsNotNone(k.parse_fetched_at("2026-11-22T21:43:32+09:00"))
        for bad in ["2026-11-22 21:43:32", "2026-11-22T21:43:32", "2026-11-22T21:43:32Z",
                    "2026-11-22T21:43:32.5+09:00", "", None, 123]:
            with self.subTest(bad=bad):
                self.assertIsNone(k.parse_fetched_at(bad))


class TempDirCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def good_cache(self, **override: object) -> dict[str, object]:
        cache: dict[str, object] = {
            "year": 2026,
            "fetched_at": "2026-11-23T09:00:00+09:00",
            "source": k.HOLIDAY_URL,
            "holidays": SAMPLE_HOLIDAYS,
        }
        cache.update(override)
        return cache


class CacheTest(TempDirCase):
    def test_missing_is_none(self):
        self.assertIsNone(k.load_cache(2026, self.dir))

    def test_good_cache_loads(self):
        write_cache(self.dir, 2026, self.good_cache())
        self.assertEqual(k.load_cache(2026, self.dir)["holidays"], SAMPLE_HOLIDAYS)

    def test_bad_caches_raise(self):
        cases = {
            "깨진 파일": "garbage",
            "빈 목록": self.good_cache(holidays=[]),
            "연도 다름": self.good_cache(year=2025),
            "받은 시각 형식 틀림": self.good_cache(fetched_at="2026-11-23 09:00:00"),
            "받은 시각 시간대 없음": self.good_cache(fetched_at="2026-11-23T09:00:00"),
            "필드 빠짐": self.good_cache(holidays=[{"calnd_dd": "2026-01-01"}]),
        }
        for name, obj in cases.items():
            with self.subTest(name):
                write_cache(self.dir, 2026, obj)
                with self.assertRaises(k.KrxError):
                    k.load_cache(2026, self.dir)

    def test_save_then_load_roundtrip_and_no_tmp_left(self):
        k.save_holidays(2026, SAMPLE_HOLIDAYS, self.dir, dt("2026-11-23T00:00:00+00:00"))
        cache = k.load_cache(2026, self.dir)
        self.assertEqual(cache["fetched_at"], "2026-11-23T09:00:00+09:00")
        self.assertEqual(cache["holidays"], SAMPLE_HOLIDAYS)
        self.assertEqual(os.listdir(self.dir), ["krx_holidays_2026.json"])

    def test_save_refuses_empty(self):
        with self.assertRaises(k.KrxError):
            k.save_holidays(2026, [], self.dir)
        self.assertEqual(os.listdir(self.dir), [])


class GetHolidaysTest(TempDirCase):
    NOW = dt("2026-11-25T18:00:00+09:00")  # 수요일. 마지막 갱신 기준 = 11-23 월 0시

    def test_miss_fetches_and_saves(self):
        fetch = FakeFetcher(SAMPLE_HOLIDAYS)
        _, fetched_at, note = k.get_holidays(2026, fetcher=fetch, directory=self.dir, now=self.NOW)
        self.assertEqual((fetch.calls, note, fetched_at), (1, k.NOTE_NEW, "2026-11-25T18:00:00+09:00"))
        self.assertIsNotNone(k.load_cache(2026, self.dir))

    def test_fresh_hit_does_not_fetch(self):
        write_cache(self.dir, 2026, self.good_cache(fetched_at="2026-11-23T09:00:00+09:00"))
        fetch = FakeFetcher(RuntimeError("호출되면 안 됨"))
        _, _, note = k.get_holidays(2026, fetcher=fetch, directory=self.dir, now=self.NOW)
        self.assertEqual((fetch.calls, note), (0, k.NOTE_CACHED))

    def test_stale_refetches_and_saves(self):
        write_cache(self.dir, 2026, self.good_cache(fetched_at="2026-11-22T09:00:00+09:00"))
        fetch = FakeFetcher(SAMPLE_HOLIDAYS[:2])
        holidays, fetched_at, note = k.get_holidays(2026, fetcher=fetch, directory=self.dir, now=self.NOW)
        self.assertEqual((fetch.calls, note, len(holidays)), (1, k.NOTE_REFRESHED, 2))
        self.assertEqual(k.load_cache(2026, self.dir)["fetched_at"], fetched_at)

    def test_stale_with_failing_fetch_errors_and_keeps_file(self):
        path = write_cache(self.dir, 2026, self.good_cache(fetched_at="2026-11-22T09:00:00+09:00"))
        before = path.read_bytes()
        fetch = FakeFetcher(k.KrxError("KRX 다운"))
        with self.assertRaises(k.KrxError):
            k.get_holidays(2026, fetcher=fetch, directory=self.dir, now=self.NOW)
        self.assertEqual(path.read_bytes(), before)

    def test_future_year_is_not_saved(self):
        fetch = FakeFetcher([{"calnd_dd": "2027-01-01", "kr_dy_tp": "금요일", "holdy_nm": "신정"}])
        _, _, note = k.get_holidays(2027, fetcher=fetch, directory=self.dir, now=self.NOW)
        self.assertEqual((fetch.calls, note), (1, k.NOTE_PREVIEW))
        self.assertEqual(os.listdir(self.dir), [])


class RoundTripTest(unittest.TestCase):
    """CI에서 Rust와 저장 파일을 주고받는 확인용. 환경 변수가 있을 때만 돈다."""

    @unittest.skipUnless(os.environ.get("KRX_ROUNDTRIP_WRITE"), "KRX_ROUNDTRIP_WRITE 없음")
    def test_write_for_rust(self):
        k.save_holidays(2026, SAMPLE_HOLIDAYS, Path(os.environ["KRX_ROUNDTRIP_WRITE"]),
                        dt("2026-11-23T09:00:00+09:00"))

    @unittest.skipUnless(os.environ.get("KRX_ROUNDTRIP_READ"), "KRX_ROUNDTRIP_READ 없음")
    def test_read_from_rust(self):
        cache = k.load_cache(2026, Path(os.environ["KRX_ROUNDTRIP_READ"]))
        self.assertIsNotNone(cache)
        self.assertEqual(cache["holidays"], SAMPLE_HOLIDAYS)
        self.assertEqual(cache["fetched_at"], "2026-11-23T09:00:00+09:00")


if __name__ == "__main__":
    unittest.main()
