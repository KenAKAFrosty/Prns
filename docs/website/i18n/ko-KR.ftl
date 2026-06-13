# 내비게이션
nav-contributing = 기여
nav-crates = Crates
nav-api = API

# 푸터
footer-tagline = Personal 팀이 만듭니다.

# 랜딩
landing-kicker = 사람들을 위한 멈추지 않는 메시 네트워크
landing-kicker-prefix = 사람들을 위한 멈추지 않는 메시 네트워크
landing-title = 안전한 Rust로 작성한 프로덕션급 Reticulum(RNS) 포트.
landing-subtitle = 결정적이고 no_std이며 할당자가 필요 없는 코어. 5달러짜리 마이크로컨트롤러부터 클라우드 서버까지, 모든 Reticulum 노드에 필요한 성능과 안정성을 위해 만들었습니다.
landing-cta-ethos = crate 선택하기
landing-cta-contributing = 기여하기

# 인용
landing-quote-label = 우리가 향해 만드는 것
landing-quote-body = Reticulum은 우리 모두가 함께 만들어 간다면 가질 수 있는 밝은 미래의 기반 통신 인프라입니다. 이것은 RNS를 더 많은 builder의 손에 쥐여 주고 그 미래를 실현하는 데 보태려는 Personal 팀의 노력입니다.

# 믿을 수 있는 기준
standards-section-label = 우리의 기준
standards-section-title = 믿을 수 있는 것
standards-license-label = 라이선스
standards-license-headline = MIT / Apache 2.0
standards-license-body = 이중 라이선스이며 permissive합니다. copyleft나 상업적 제한이 없습니다.
standards-safety-label = 안전성
standards-safety-headline = #![forbid(unsafe_code)]
standards-safety-body = 우리의 crate에는 unsafe가 전혀 없고, 컴파일러가 이를 강제합니다. 의존성 안의 unsafe는 Miri로 UB를 확인하고 cargo-geiger로 감사합니다.
standards-correctness-label = 정확성
standards-correctness-headline = RNS와 diff 테스트
standards-correctness-body = 모든 변경은 레퍼런스와 대조한 뒤 property, fuzz, mutation 테스트를 거치고, 중요한 곳에는 Kani 증명을 둡니다.
standards-benchmarked-label = 성능
standards-benchmarked-headline = 주장보다 측정
standards-benchmarked-body = 성능은 공개적으로 추적되며, 직접 실행할 수 있는 harness로 측정됩니다.
standards-benchmarked-cta = 벤치마크 보기 →

# 어디서 시작할까요?
start-section-label = 시작 경로
start-section-title = 어디서 시작할까요?
start-section-lead = 만들고 있는 것에 맞는 경로를 고르세요. 지금은 각 경로가 하나의 crate로 이어지며, 더 많은 가이드가 함께 추가될 예정입니다.

start-daemon-headline = Reticulum 노드를 실행하고 싶어요
start-daemon-body = 미리 빌드된 daemon입니다. rnsd의 drop-in입니다. 이미 가지고 있는 노드 옆에서 실행하세요.
start-daemon-code = apt install personal-rnsd
start-daemon-target = personal-rnsd

start-mobile-headline = 모바일 앱을 만들고 있어요
start-mobile-body = Kotlin(.aar), Swift(.xcframework), Python(.whl) — daemon이 쓰는 것과 같은 엔진을 앱 안에 직접 넣습니다.
start-mobile-code = implementation("org.staypersonal:rns:0.1")
    pod 'PersonalRns', '~> 0.1'
start-mobile-target = personal-rns-ffi

start-game-headline = 게임에 넣어 출시하려 해요
start-game-body = Unity, Godot, MonoGame용 C# / .NET 바인딩. 서버를 세우지 않는 멀티플레이어.
start-game-code = dotnet add package Personal.Rns
start-game-target = personal-rns-ffi

start-embedded-headline = 마이크로컨트롤러를 대상으로 해요
start-embedded-body = 엔진과 세 메서드만 있는 Host trait. ESP32-C6가 기준이며 S3, nRF, RP2040, STM32가 다음입니다.
start-embedded-code = cargo add personal-rns --no-default-features
start-embedded-target = personal-rns + personal-hopspot

start-web-headline = 웹이나 edge용으로 만들고 있어요
start-web-body = 브라우저와 Cloudflare Workers, Fastly, Spin 같은 edge runtime에서 실행되는 WebAssembly 빌드입니다.
start-web-code = npm install personal-rns
start-web-target = personal-rns (wasm32)

start-rust-headline = Rust 앱에 임베드하고 싶어요
start-rust-body = 바로 쓸 수 있는 완전한 RNS runtime, 또는 직접 runtime을 둘러 만들 수 있는 순수 코어.
start-rust-code = cargo add personal-rnsd   # complete RNS runtime
    cargo add personal-rns      # pure core only
start-rust-target = personal-rnsd or personal-rns

start-lxmf-headline = 메시 위로 메시지를 보내고 싶어요
start-lxmf-body = Reticulum 위의 LXMF — identity, address, delivery. Sideband와 Nomadnet이 올라가는 계층입니다.
start-lxmf-code = cargo add personal-lxmf
start-lxmf-target = personal-lxmf

# 플랫폼 ("Runs on") — hero marquee label + CTA, and the dedicated page
landing-platforms-label = 실행 환경
landing-platforms-cta = 모두 보기 →
platforms-title = Prns가 실행되는 곳
platforms-lead = 하나의 엔진, 여러 보금자리. 일부는 오늘 제공되고, 나머지는 로드맵에 있습니다 — 우리가 향해 만드는 북극성입니다. 채워진 칩은 지금 실행되고, 점선은 다음 차례입니다.
platforms-legend-shipping = 현재 제공
platforms-legend-roadmap = 로드맵

# 벤치마크 페이지
benchmarks-kicker = 성능
benchmarks-title = 공개 벤치마크
benchmarks-lead = 우리는 성능을 형용사가 아니라 숫자로 다룹니다. 여기의 모든 수치는 repo 안의 결정적 harness에서 나오며, 실제 하드웨어에서 측정하고 비교가 공정한 곳에서는 RNS 레퍼런스와 대조했습니다. 수치는 suite가 안정화되는 동안 채워지고 있습니다. 아래에는 그 수치들이 따르는 방법론이 있습니다.

# 라이선스 신호 (푸터)
footer-license = 오픈 소스. MIT / Apache 2.0.

# 기여 페이지
contributing-kicker = 기준선
contributing-title = 기여
contributing-lead = 기여하는 방법 — 우리가 중요하게 여기는 것, 코드가 따르는 관례, 모든 변경이 통과해야 하는 기준입니다. 사람 기여자와 자동화된 기여자 모두에게 적용됩니다.

# Crates index
crates-kicker = 구성 요소
crates-title = 만들고 있는 것에 맞는 것을 고르세요.
crates-lead = 각 crate는 나머지를 가져오지 않아도 그 자체로 유용하도록 만들었습니다. 엔진이 기반이고, 나머지는 그 위에 쌓이며, suite가 커지면서 더 많은 조각이 추가됩니다.
crates-card-cta = 무엇을 하는지 →
crates-back = 모든 crates
crates-not-found = 그런 이름의 crate가 없습니다

# crate별 카드
crate-rns-role = 엔진
crate-rns-blurb = 어떤 Rust 프로젝트에도 Reticulum을 넣으세요. 결정적이고 no_std이며 할당자가 없습니다. 전역 상태도, 내장 I/O도 없습니다 — clock과 wire는 직접 가져오면 됩니다.
crate-rnsd-role = daemon
crate-rnsd-blurb = Linux가 도는 곳이면 어디서든 실행되는 rnsd drop-in입니다. RNS 레퍼런스와 같은 wire입니다. 이미 가진 노드 옆에서 또는 대신 사용하세요.
crate-lxmf-role = 메시징
crate-lxmf-blurb = Reticulum 위의 LXMF — Sideband와 Nomadnet이 올라가는 계층. identity, address, message delivery.
crate-ffi-role = 모바일 + Python 바인딩
crate-ffi-blurb = 하나의 uniffi interface가 Kotlin(.aar), Swift(.xcframework), Python(.whl)을 생성합니다. Android, iOS, Jupyter notebook에서 Reticulum을 사용하세요 — 같은 형태, 같은 엔진입니다.

# 404
not-found-title = 아직 여기는 비어 있습니다.
not-found-cta = 홈으로 돌아가기
