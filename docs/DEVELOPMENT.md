# 개발 지시서 — dps150-ui

이 문서는 [기획서](PLAN.md)를 실제 코드로 옮길 때 지킬 규칙과 작업 순서를 정한다. 기획서가 *무엇을* 만들지 정한다면, 이 문서는 *어떻게* 만들고 언제 끝났다고 볼지를 정한다.

## 1. 개발 환경

| 항목 | 값 |
| --- | --- |
| Rust | stable 1.98 이상, Windows는 `x86_64-pc-windows-msvc` |
| 빌드 | `cargo build --release` (워크스페이스 루트에서) |
| 실행 | `cargo run -p dps150-ui --release` |
| 모의 장비로 실행 | 앱 상단의 포트 목록에서 「모의 장비」 선택 |
| 테스트 | `cargo test --workspace` |
| 정적 검사 | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` |

Windows에서 `cargo`가 PATH에 없으면 `%USERPROFILE%\.cargo\bin`을 PATH에 추가한다.

## 2. 저장소 구조

```
power-supply/
├─ Cargo.toml              # [workspace]
├─ crates/
│  ├─ dps150-proto/        # 프레임·파서. I/O 없음, 의존성 최소
│  ├─ dps150-device/       # Transport, 시리얼, I/O 스레드, DeviceHandle
│  ├─ dps150-sim/          # 모의 장비 (Transport 구현)
│  └─ dps150-ui/           # eframe 앱
└─ docs/
   ├─ PLAN.md              # 기획서
   └─ DEVELOPMENT.md       # 이 문서
```

의존 방향은 `ui → device → proto`, `sim → proto`, `ui → sim`이다. `proto`는 다른 crate에 의존하지 않는다. 역방향 의존은 만들지 않는다.

## 3. crate별 책임과 공개 API

### 3.1 `dps150-proto`

- `encode(cmd, ty, payload) -> Vec<u8>`: 송신 프레임(`0xF1` 헤더)을 만든다. 체크섬 = (타입 + 길이 + 페이로드 합) mod 256
- 명령 헬퍼: `session(open)`, `set_baud(index)`, `get(ty)`, `set_f32(ty, v)`, `set_u8(ty, v)`
- `FrameDecoder`: 바이트 조각을 받아 완성된 `Frame`을 내놓는다. `0xF0`이 아닌 바이트는 버리고, 체크섬이 틀리면 1바이트 밀어서 다시 맞춘다. **조각이 프레임 중간에서 끊겨도 데이터를 잃지 않아야 한다**
- `Message::from_frame(&Frame)`: 타입 코드별 의미 있는 값으로 바꾼다. 모르는 타입은 `Message::Unknown`
- `FullState`: 0xFF 페이로드 파서와 역직렬화(`to_payload`). 오프셋은 기획서 표를 따른다
- `DataType` 상수: 기획서 「데이터 타입」 표의 코드를 그대로 쓴다

### 3.2 `dps150-device`

- `trait Transport: Send { fn read(&mut self, buf) -> io::Result<usize>; fn write_all(&mut self, data) -> io::Result<()>; }`. `read`는 최대 50 ms 안에 반환해야 한다(데이터가 없으면 0)
- `SerialTransport::open(port)`: 115200 8N1, DTR off, RTS on. 실패하면 300 ms 간격으로 최대 3회 다시 연다(Windows 오류 995 대응)
- `find_ports()`: 사용 가능한 포트 목록. VID `2E3C` / PID `5740`이면 `is_dps150 = true`
- `Device::spawn(transport, notify) -> DeviceHandle`: I/O 스레드를 띄운다. `notify`는 새 데이터가 오면 호출하는 콜백(UI의 `request_repaint`)
- `DeviceHandle`
  - `send(Command)`: 명령을 큐에 넣는다. 막히지 않는다
  - `state() -> DeviceState`: 최신 스냅샷 복사본
  - `subscribe() -> Receiver<Event>`: 이벤트 구독. 구독자는 여럿일 수 있다(UI, 기록기)
  - `shutdown(output_off: bool)`: 출력 OFF(선택) → 세션 종료 → 스레드 종료
- I/O 스레드 동작
  1. 연결 절차: 세션 시작 → 보레이트(인덱스 5) → 주소·모델·FW·HW·전체 상태 요청
  2. 수신 프레임으로 `DeviceState`를 갱신하고 `Event`를 발행한다
  3. 1초마다 전체 상태(0xFF)를 요청한다
  4. 3초 동안 프레임이 하나도 없으면 `Event::Disconnected`를 보내고 종료한다
- `Command::EnableOutput { v, i }`: V·I를 먼저 보내고, 장비가 돌려준 설정값이 ±0.01 V / ±0.001 A 안에 들어온 것을 확인한 뒤에만 ON을 보낸다. 1초 안에 확인되지 않으면 ON을 보내지 않고 `Event::Error`를 낸다

### 3.3 `dps150-sim`

- `SimTransport::new(SimConfig)`: 장비 프로토콜에 응답하는 가짜 장비. 출력 V·I·P 프레임을 200 ms마다 보낸다
- 부하 모델은 저항 R(기본 10 Ω)이다. CV일 때 I = V / R, I가 설정 전류를 넘으면 CC로 바뀌어 I = 설정 전류, V = I × R
- 보호: V > OVP, I > OCP, P > OPP이면 보호 상태를 바꾸고 출력을 끈다
- 테스트에서 끊김을 흉내 낼 수 있게 `SimControl`로 응답을 멈추거나 부하를 바꿀 수 있다

### 3.4 `dps150-ui`

| 모듈 | 책임 |
| --- | --- |
| `app.rs` | `eframe::App`, 패널 배치, 페이지 전환, 이벤트 처리 |
| `pages/*.rs` | 페이지별 그리기: dashboard, automation, presets, device, log, settings |
| `automation.rs` | `Step` 계획 생성(스윕·시퀀스), 실행기 스레드 |
| `recorder.rs` | CSV 기록기 스레드, 그래프용 링버퍼 |
| `config.rs` | 앱 설정 저장·불러오기, 공식 앱 config.json 가져오기 |
| `i18n.rs` | 한국어·영어 문자열 |
| `fonts.rs` | 한글 글꼴 로드 |

## 4. 반드시 지킬 안전 규칙

아래 규칙은 리뷰에서 하나라도 어기면 병합하지 않는다.

1. **출력 ON은 `Command::EnableOutput`으로만 보낸다.** UI나 자동화 코드에서 출력 ON 프레임을 직접 만들지 않는다
2. **입력란은 키를 누를 때마다 전송하지 않는다.** Enter나 「적용」 버튼으로만 전송한다
3. **보내기 전에 클램프한다.** 전압은 `min(최대 V, OVP)`, 전류는 `min(최대 A, OCP)`로 자르고, 잘렸으면 화면에 알린다
4. **출력을 켜는 모든 동작은 확인을 받는다.** 확인창에 V·I 값을 크게 표시한다. 「다시 묻지 않기」는 앱을 다시 켜면 초기화한다
5. **자동화는 끝나거나 멈추면 출력을 끈다.** 중단, 보호 동작, 연결 끊김, 앱 종료 모두 해당한다
6. **비상 OFF는 어디서나 동작한다.** 상단 버튼, `F12`, `Esc` 두 번
7. **앱을 닫으면 출력을 끄고 세션을 종료한다.** 설정에서 끌 수 있지만 기본값은 켜짐이다
8. **부트로더 진입(0xC0)은 코드에 넣지 않는다**

## 5. 코딩 규칙

- `unwrap()`·`expect()`는 테스트와 「일어날 수 없음」이 명백한 곳에서만 쓴다. I/O와 파싱 오류는 `Result`로 올린다
- 스레드 사이 공유는 채널(`crossbeam-channel`)과 `Arc<RwLock<_>>`만 쓴다. UI 스레드에서 시리얼 포트를 만지지 않는다
- UI 스레드에서 블로킹 호출을 하지 않는다(`sleep`, 포트 열기, 파일 대화상자 제외 파일 I/O)
- 모든 공개 타입·함수에 한 줄 문서 주석을 단다
- 로그는 `log` crate로 남긴다. 사용자에게 보여줄 사건은 이벤트 로그 페이지에도 넣는다
- 숫자 단위는 이름에 붙인다(`volts`, `amps`, `hold_secs`) 또는 타입 문서에 적는다

## 6. 작업 순서와 완료 기준

| 단계 | 할 일 | 완료 기준 |
| --- | --- | --- |
| M0 | 워크스페이스, `dps150-proto`와 단위 테스트 | `cargo test -p dps150-proto` 통과. 실기에서 받은 전체 상태 프레임이 파싱됨 |
| M1 | `dps150-device`, `dps150-sim`, 통합 테스트, 실기 읽기 테스트 | 시뮬레이터로 연결·끊김·보호·EnableOutput 검증 테스트 통과. 실기에서 상태 읽기 성공 |
| M1-W | 실기 쓰기 검증 | 무부하에서 3.3 V / 0.1 A 설정 → ON → 측정 → OFF 확인. **출력 단자 연결 상태를 사용자가 확인한 뒤에만 진행** |
| M2 | UI MVP: 대시보드, 측정 패널, 확인창, 비상 OFF, 그래프, CSV 기록, 모의 장비 모드 | 모의 장비로 전 기능 동작. 1시간 연속 기록 시 메모리 증가 없음 |
| M3 | 프리셋, 보호값, 밝기·적산, 스윕, 시퀀스, 이벤트 로그, 설정, 공식 config 가져오기 | 시뮬레이터에서 중단·보호·끊김 시 출력 OFF 확인 |
| M4 | 배포: 3개 OS 빌드, xlsx, CLI | CI 녹색, 릴리스 바이너리 업로드 |

각 단계가 끝나면 `cargo fmt`, `cargo clippy -D warnings`, `cargo test`를 모두 통과시킨 뒤 커밋한다.

## 7. 테스트 지침

- `proto`: 인코딩↔디코딩 왕복, 체크섬, 바이트 조각을 임의 위치에서 나눠 넣어도 같은 프레임이 나오는지, 쓰레기 바이트 뒤 재동기화
- `device` + `sim`: 연결 직후 장비 정보 수신, `EnableOutput` 성공·실패 경로, 3초 무응답 시 `Disconnected`, 보호 이벤트
- `automation`: 스윕 계획 경계값(시작=끝, 단계>범위, 음수, 상한 초과), 중단 시 마지막 명령이 출력 OFF인지
- 실기 테스트는 `cargo test -- --ignored`로만 돈다. 환경 변수 `DPS150_PORT`로 포트를 지정하고, 출력을 켜는 테스트는 `DPS150_ALLOW_OUTPUT=1`이 있을 때만 실행한다

## 8. 커밋과 브랜치

- `main`에 직접 커밋할 때도 빌드·테스트가 통과한 상태여야 한다
- 커밋 메시지는 영어 명령형 한 줄 요약 + 필요하면 본문
- 공식 앱의 바이너리, 디컴파일 코드, 이미지 자원은 저장소에 넣지 않는다
