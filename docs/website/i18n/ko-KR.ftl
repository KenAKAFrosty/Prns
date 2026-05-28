# 내비게이션
nav-ethos = 디자인
nav-crates = 크레이트
nav-api = API

# 푸터
footer-tagline = Personal 팀이 만들었습니다.

# 랜딩
landing-kicker = 막을 수 없는 메시 네트워크, 사람들을 위한
landing-title = Rust로 작성된 Reticulum(RNS)의 프로덕션급 포팅.
landing-subtitle = 결정론적이고 no_std에 할당자가 없는 코어. RNS와 LXMF를 빠짐없이 지원합니다. Kotlin, Swift, Python, TypeScript, C# 네이티브 바인딩. 브라우저와 엣지 런타임을 위한 WebAssembly. 5달러짜리 마이크로컨트롤러부터 클라우드 노드까지, 모든 Reticulum 스택이 요구하는 성능과 배터리 수명을 위해 만들어졌습니다. rnsd의 드롭인 대체품도 포함됩니다.
landing-cta-ethos = 크레이트 고르기
landing-cta-crates = 어떻게 만드는지 보기

# 인용
landing-quote-label = 우리가 향하는 곳
landing-quote-body = Reticulum은 우리가 만들어낼 수 있는 밝은 미래의 근간이 되는 통신 인프라입니다. 이 프로젝트는 그것을 더 많은 개발자의 손에 쥐여 주어 그 미래를 함께 실현해 가려는 노력입니다.

# 신뢰 카드
standards-section-label = 우리의 기준
standards-section-title = 믿고 의지할 수 있는 것
standards-license-label = 라이선스
standards-license-headline = MIT / Apache 2.0
standards-license-body = 듀얼 라이선스, 관대한 조건. copyleft도, 비상업적 제한도 없습니다.
standards-coverage-label = 커버리지
standards-coverage-headline = RNS와 LXMF 완전 지원
standards-coverage-body = RNS만이 아닙니다. LXMF가 곁다리도 아닙니다. 둘 다, 완전히.
standards-core-label = 코어
standards-core-headline = no_std, 할당자 없음
standards-core-body = 할당자가 동작하지 못하는 환경에서도 굴러가는 결정론적 코어.
standards-verification-label = 검증
standards-verification-headline = RNS 레퍼런스 대조 테스트
standards-verification-body = 모든 변경 사항은 레퍼런스와 비교 검증됩니다. 필요할 때는 정형 증명까지.

# 어디서부터 시작할까?
start-section-label = 진입로
start-section-title = 어디서부터 시작할까?
start-section-lead = 만들고 있는 것에 맞는 경로를 고르세요. 지금은 각 항목이 하나의 크레이트로 연결되지만, 가이드도 곧 함께 도착합니다.

start-daemon-headline = Reticulum 노드를 띄우고 싶어요
start-daemon-body = 미리 빌드된 데몬. rnsd 드롭인. 기존 노드 옆에서 그대로 돌리세요.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = 모바일 앱을 만들고 있어요
start-mobile-body = Kotlin (.aar), Swift (.xcframework), Python (.whl) — 데몬과 동일한 엔진을 앱 안에 직접 임베드합니다.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = 게임에 탑재하려고 합니다
start-game-body = Unity, Godot, MonoGame을 위한 C# / .NET 바인딩. 서버 없이도 멀티플레이.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = 마이크로컨트롤러에 올리려고 합니다
start-embedded-body = 엔진과 세 개의 메서드로 이루어진 Host 트레이트. ESP32-C6가 레퍼런스이며 S3, nRF, RP2040, STM32가 이어집니다.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + hosts/*

start-web-headline = 웹이나 엣지를 노리고 있어요
start-web-body = 브라우저와 Cloudflare Workers, Fastly, Spin 같은 엣지 런타임에서 실행되는 WebAssembly 빌드.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Rust 앱에 임베드합니다
start-rust-body = 박스에서 꺼내 바로 쓰는 완전한 RNS 런타임, 또는 직접 런타임을 짜기 위한 순수 코어.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = 메시 위로 메시지를 보내고 싶어요
start-lxmf-body = Reticulum 위의 LXMF — 신원, 주소, 전달. Sideband와 Nomadnet이 자리 잡은 그 계층.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# 라이선스 (푸터)
footer-license = 오픈 소스. MIT / Apache 2.0.

# 설계 사상
ethos-kicker = 우리의 원칙
ethos-title = 어떻게 만드는가
ethos-lead = 이 프로젝트의 작업 원칙을 엔지니어가 엔지니어에게 풀어놓는 글입니다. 순수한 엔진, 할당자 없는 코어, 모든 변경은 RNS 레퍼런스 대조로 검증. 의존하기 전에 훑어보세요. 어떤 프로젝트에 발을 들이는지 알아두는 편이 좋습니다.

# 크레이트
crates-kicker = 구성 요소
crates-title = 만들고 있는 것에 맞춰 고르세요.
crates-lead = 각 크레이트는 다른 것들 없이도 단독으로 쓸 수 있게 설계되었습니다. 엔진이 기반이며 나머지는 그 위에 쌓입니다. 스위트가 자라면서 더 많은 조각이 추가될 예정입니다.
crates-card-cta = 무엇을 하는지 →
crates-back = 전체 크레이트
crates-not-found = 그런 이름의 크레이트는 없습니다

# 각 크레이트 카드
crate-rns-role = 엔진
crate-rns-blurb = 어떤 Rust 프로젝트에든 Reticulum을 끼워 넣으세요. 결정론적, no_std, 할당자 없음. 전역 상태도, 내장 I/O도 없습니다 — 시계와 통신로는 직접 가져오세요.
crate-rnsd-role = 데몬
crate-rnsd-blurb = Linux가 돌아가는 곳이면 어디서든 동작하는 rnsd 드롭인. RNS 레퍼런스와 동일한 와이어, 기존 노드 옆이든 자리 바꿔 쓰든 자유입니다.
crate-lxmf-role = 메시징
crate-lxmf-blurb = Reticulum 위의 LXMF — Sideband와 Nomadnet이 자리 잡은 계층. 신원, 주소, 메시지 전달.
crate-ffi-role = 모바일 + 파이썬 바인딩
crate-ffi-blurb = 하나의 uniffi 인터페이스가 Kotlin (.aar), Swift (.xcframework), Python (.whl)을 만들어냅니다. Android, iOS, Jupyter 노트북에서 동일한 모양과 동일한 엔진으로 Reticulum을 사용하세요.
crate-rvt-role = 시각적 디버거
crate-rvt-blurb = 가상 시계 위에서 시뮬레이션된 노드 사이로 패킷이 움직이는 모습을 관찰하세요. 결정론적 — 같은 시나리오, 같은 추적, 항상 같은 결과.
crate-esp32c6-role = ESP32-C6 펌웨어
crate-esp32c6-blurb = ESP32-C6용 베어메탈 호스트 어댑터. OS도, 할당자도 없습니다 — 무선 라디오가 내장된 5달러짜리 RISC-V 칩에서 엔진이 동작한다는 증거입니다.

# 404
not-found-title = 여기에는 아직 아무것도 없어요.
not-found-cta = 홈으로 돌아가기
