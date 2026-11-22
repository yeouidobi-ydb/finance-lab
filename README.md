# finance-lab

금융 데이터·분석 코드와 AI 스킬을 모아 두는 메인 저장소.

## 규칙

- **모든 도구는 Python과 Rust 두 가지로 작성한다.** 같은 입력이면 같은 결과가 나와야 한다.
- 도구 하나에 폴더 하나: `tools/<도구이름>/` 아래에 `.py`, `.rs`, `Cargo.toml`, `README.md`를 둔다.
- 데이터 파일(xlsx, csv 등)과 API 키는 커밋하지 않는다(`.gitignore` 참고).

## 구조

```
tools/          금융 데이터 수집·분석 도구 (Python + Rust)
  krx_schedule/   KRX 휴장일 및 장운영 공지 조회
skills/         Claude 스킬 (폴더마다 SKILL.md 하나)
```

## 도구 목록

| 도구 | 설명 |
|---|---|
| [krx_schedule](tools/krx_schedule/) | 특정 날짜가 KRX 휴장일인지와 관련 공지를 조회 |
