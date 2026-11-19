# power-supply

FNIRSI DPS-150 USB-C 프로그래머블 전원 공급기를 제어하는 Rust + egui 데스크톱 앱 프로젝트입니다.

현재는 기획 단계입니다. 기획서는 [docs/PLAN.md](docs/PLAN.md)에 있습니다.

## 목표

- 공식 Windows 앱을 대체하는 크로스플랫폼(Windows·macOS·Linux) 단일 실행 파일
- 실시간 측정, 전압·전류 설정, 출력 제어, 스윕·시퀀스 자동화
- 실시간 그래프와 CSV 기록
- 출력 켜기 확인, 한계값 클램프, 연결이 끊기면 출력 OFF 같은 안전장치

## 계획된 구성

| crate | 역할 |
| --- | --- |
| `dps150-proto` | 시리얼 프레임 인코딩·디코딩, 상태 파서 |
| `dps150-device` | 시리얼 I/O 스레드, 연결 관리 |
| `dps150-sim` | 장비 없이 테스트하기 위한 모의 장비 |
| `dps150-ui` | eframe/egui 데스크톱 앱 |
