# 내비게이션
nav-benchmarks = 벤치마크
nav-contributing = 기여
site-early-english-note = Prns는 아직 초기 단계입니다. 전체 문서는 GitHub와 소스 코드에 있으며, 아직은 영어로만 제공됩니다.

# 푸터
footer-tagline = KenAKAFrosty와 Personal/Prns 팀이 만듭니다.
footer-flash = Hopspot 플래시하기 (영어만 제공)
footer-playground = 브라우저 플레이그라운드 (영어만 제공)

# 랜딩
landing-kicker = 당신의 것이 되는 메시 네트워크
landing-kicker-prefix = 당신의 것이 되는 메시 네트워크
landing-title = 어떤 기기에서도 돌아가도록 만든 고성능 Reticulum(RNS).
landing-title-lead = 고성능 Reticulum(RNS),
landing-title-accent = 어떤 기기에서도 돌아가도록.
landing-subtitle = 5달러짜리 마이크로컨트롤러부터 클라우드 서버 클러스터까지, 모든 Reticulum 노드에 필요한 성능, 안정성, 에너지 효율을 위해 만들었습니다. 하나의 엔진과 하나의 API가 임베디드, 데스크톱, 모바일, 게임, 웹에서 똑같이 동작합니다.
landing-cta-ethos = Prns에서 나의 길 찾기
landing-cta-standards = 우리의 기준
# 인용
landing-quote-label = 우리가 만들어 가려는 것
landing-quote-body = Reticulum은 우리 모두가 함께 만들어 간다면 가질 수 있는 밝은 미래의 기반 통신 인프라입니다. 이것은 RNS를 더 많은 builder의 손에 쥐여 주고 그 미래를 실현하는 데 보태려는 Personal 팀의 노력입니다.

# 인터페이스
interfaces-section-label = 인터페이스
interfaces-section-title = 메시가 현실 세계와 만나는 지점
interfaces-section-lead = Prns는 builder가 이미 아는 RNS 호환 인터페이스를 유지하고, 새로운 기기와 네트워크를 위한 네이티브 링크로 지도를 넓힙니다.
interfaces-section-hot-note = Prns 인터페이스는 hot-swappable입니다. 노드를 재시작하지 않고 인터페이스를 추가, 제거 또는 변경할 수 있습니다.

interfaces-radio-label = 무선
interfaces-radio-headline = 기기와 보드를 위한 근거리 링크
interfaces-radio-body = Bluetooth LE Auto-interface, ESP-NOW, LoRa가 가까운 기기, 보드 플릿, 장거리 RF 링크를 하나의 Reticulum 메시로 연결합니다.

interfaces-lan-label = LAN
interfaces-lan-headline = 자동 발견되는 로컬 링크 피어
interfaces-lan-body = Wi-Fi Auto-interface는 multicast, mDNS, gateway rendezvous로 가까운 노드를 찾고 로컬 네트워크를 메시로 끌어들입니다.

interfaces-cable-label = 케이블 + 패킷 라디오
interfaces-cable-headline = 케이블, TNC, 라디오 모뎀
interfaces-cable-body = USB Auto-interface, serial framing, KISS, AX.25, RNode가 작은 기기와 패킷 라디오 하드웨어를 같은 메시에 연결합니다.

interfaces-host-label = 라우팅된 IP
interfaces-host-headline = Internet, WAN, backbone 링크
interfaces-host-body = TCP client/server, UDP, WebSocket, Backbone은 먼 피어도 private WAN, VPN, public Internet relay, 브라우저 통합을 거쳐 메시에 참여하게 합니다.

# 믿을 수 있는 기준
standards-section-label = 우리의 기준
standards-section-title = 믿을 수 있는 것
standards-license-label = 라이선스
standards-license-headline = MIT / Apache 2.0
standards-license-body = 퍼미시브한 이중 라이선스입니다. copyleft나 상업적 제한이 없습니다.
standards-safety-label = 안전성
standards-safety-headline = 먼저 강제, 그다음 감사
standards-safety-body = 엔진에서는 panic, unwrap, 근거 없는 unsafe가 결코 컴파일되지 않습니다. 금지할 수 없는 것은 감사합니다. 의존성 안의 unsafe는 cargo-geiger로, 정의되지 않은 동작은 Miri로, 보안 권고는 cargo-deny로 확인합니다.
standards-correctness-label = 정확성
standards-correctness-headline = RNS와 diff 테스트
standards-correctness-body = 모든 변경은 레퍼런스와 대조한 뒤 unit, property, fuzz, mutation 테스트를 거치고, 중요한 곳에는 Kani 증명을 둡니다.
standards-benchmarked-label = 성능
standards-benchmarked-headline = 주장보다 측정
standards-benchmarked-body = 성능은 공개적으로 추적되며, 직접 실행할 수 있는 harness로 측정됩니다.
standards-benchmarked-cta = 벤치마크 보기 →

# 어디서 시작할까요?
start-section-label = 시작 경로
start-section-title = 무엇을 하러 오셨나요?
start-section-lead = Prns가 내 작업에 들어오는 방식에 맞는 경로를 고르세요. 플래시할 하드웨어, 운영할 인프라, 만들 소프트웨어 중 하나입니다.

start-daemon-headline = daemon 실행하기
start-daemon-body = 데스크톱, LXMF 앱, backbone VPS 등을 위한 빠른 Reticulum daemon을 설치하세요.
start-daemon-code = 기존 앱에 drop-in
    ~/.reticulum 읽기
    인터페이스 라이브 편집
    메트릭 내장
start-daemon-target = Prnsd 실행

start-embedded-headline = Hopspot 플래시하기
start-embedded-body = 지원 보드를 고르고 무선과 배터리 트레이드오프를 비교한 뒤 전용 메시 기기를 플래시하세요.
start-embedded-code = 보드 매트릭스
    웹 플래셔
    로컬 플래시
start-embedded-target = Hopspot 플래시하기 (영어만 제공)

start-web-headline = 브라우저 노드 플레이그라운드 사용하기
start-web-body = WebAssembly에서 공유 Rust 엔진을 사용하는 TypeScript API를 체험하고, Auto Wi-Fi 또는 USB Auto로 연결해 로컬 노드 활동을 실시간으로 확인하세요.
start-web-code = WebAssembly 런타임
    Auto Wi-Fi + USB Auto
    TypeScript 예제
start-web-target = 플레이그라운드 열기 (영어만 제공)

start-rust-headline = Reticulum 위에 구축하기
start-rust-body = 엔진과 바인딩으로 앱, 도구, 서비스, 게임에 메시 네트워킹을 더하세요.
start-rust-target = README 읽기
start-rust-target-source = 소스 다운로드

# 플랫폼 ("Runs on") — hero marquee label + CTA, and the dedicated page
landing-platforms-label = 실행 환경
landing-platforms-cta = 전체 보기 →
platforms-title = Prns가 돌아가는 곳
platforms-lead = 하나의 엔진, 여러 보금자리. 이 요약은 런타임 플랫폼 지원과 개별 Hopspot 보드 지원을 구분해 보여 줍니다.
platforms-board-support-link = 보드 지원과 bring-up 보기 →

# Hopspot 플래시 페이지
flash-back = 플랫폼
flash-back-boards = 보드
flash-card-action = 플래시

# 벤치마크 페이지
benchmarks-kicker = 성능
benchmarks-title = 공개 벤치마크
benchmarks-lead = 아래의 모든 수치는 repo에 공개된 결과에서 나오며, 직접 실행할 수 있는 harness로 실제 하드웨어에서 측정되었습니다. 여기서부터의 내용은 아직 영어로만 제공됩니다.

# 라이선스 신호 (푸터)
footer-license = 오픈 소스. MIT / Apache 2.0.
footer-trademarks = 제3자 로고, 상표, 제품 이미지는 각 소유자에게 속합니다. 이는 플랫폼, 하드웨어, 호환성 대상을 식별하기 위해서만 표시됩니다. 보증이나 승인을 주장하거나 암시하지 않습니다.

# 404
not-found-title = 여기는 아직 비어 있습니다.
not-found-cta = 홈으로 돌아가기
