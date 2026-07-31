# 내비게이션
nav-contributing = 기여
nav-api = API

# 푸터
footer-tagline = Personal 팀이 만듭니다.

# 랜딩
landing-kicker = 사람들을 위한 멈추지 않는 메시 네트워크
landing-kicker-prefix = 사람들을 위한 멈추지 않는 메시 네트워크
landing-title = 안전한 Rust로 작성한 고성능 Reticulum(RNS) 포트.
landing-title-lead = A high-performance port of Reticulum (RNS).
landing-title-accent = Runs on any device.
landing-subtitle = 결정적이고 no_std이며 할당자가 필요 없는 코어. 5달러짜리 마이크로컨트롤러부터 클라우드 서버까지, 모든 Reticulum 노드에 필요한 성능과 안정성을 위해 만들었습니다.
landing-cta-ethos = crate 선택하기
# 인용
landing-quote-label = 우리가 향해 만드는 것
landing-quote-body = Reticulum은 우리 모두가 함께 만들어 간다면 가질 수 있는 밝은 미래의 기반 통신 인프라입니다. 이것은 RNS를 더 많은 builder의 손에 쥐여 주고 그 미래를 실현하는 데 보태려는 Personal 팀의 노력입니다.

# 인터페이스
interfaces-section-label = 인터페이스
interfaces-section-title = 메시가 현실 세계와 만나는 지점
interfaces-section-lead = Prns는 builder가 이미 아는 RNS-compatible interface를 유지하고, 새로운 기기와 네트워크를 위한 native link로 지도를 넓힙니다.
interfaces-section-hot-note = Prns 인터페이스는 hot-swappable입니다. 노드를 재시작하지 않고 인터페이스를 추가, 제거 또는 변경할 수 있습니다.

interfaces-radio-label = 무선
interfaces-radio-headline = 기기와 보드를 위한 근거리 링크
interfaces-radio-body = Bluetooth LE Auto-interface, ESP-NOW, LoRa가 가까운 기기, 보드 플릿, 장거리 링크를 하나의 Reticulum 메시로 연결합니다.

interfaces-lan-label = LAN
interfaces-lan-headline = 자동 발견되는 로컬 링크 피어
interfaces-lan-body = Wi-Fi Auto-interface는 multicast, mDNS, gateway rendezvous로 가까운 노드를 찾고 로컬 네트워크를 메시로 접어 넣습니다.

interfaces-cable-label = 케이블 + 패킷 라디오
interfaces-cable-headline = 케이블, TNC, 라디오 모뎀
interfaces-cable-body = USB Auto-interface, serial framing, KISS, AX.25, RNode가 작은 장치와 패킷 라디오 하드웨어를 같은 메시에 연결합니다.

interfaces-host-label = 라우팅된 IP
interfaces-host-headline = Internet, WAN, backbone 링크
interfaces-host-body = TCP client/server, UDP, Backbone은 먼 peer도 private WAN, VPN, public Internet relay를 거쳐 메시 참여하게 합니다.

# 믿을 수 있는 기준
standards-section-label = 우리의 기준
standards-section-title = 믿을 수 있는 것
standards-license-label = 라이선스
standards-license-headline = MIT / Apache 2.0
standards-license-body = 이중 라이선스이며 permissive합니다. copyleft나 상업적 제한이 없습니다.
standards-safety-label = 안전성
standards-safety-headline = 먼저 강제, 그다음 감사
standards-safety-body = 엔진에서는 panic, unwrap, 근거 없는 unsafe가 결코 컴파일되지 않습니다. 금지할 수 없는 것은 감사합니다. 의존성 안의 unsafe는 cargo-geiger로, 정의되지 않은 동작은 Miri로, 보안 권고는 cargo-deny로 확인합니다.
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
start-daemon-target = prnsd

start-embedded-headline = Flash a Hopspot
start-embedded-body = Pick a supported board, compare radio and battery tradeoffs, then flash a dedicated mesh device.
start-embedded-code = Board matrix
    Web flasher
    Local flash
start-embedded-target = Flash a Hopspot

start-web-headline = 브라우저 노드 플레이그라운드 사용하기
start-web-body = WebAssembly에서 공유 Rust 엔진을 사용하는 TypeScript API를 체험하고, Auto Wi-Fi 또는 USB Auto로 연결해 로컬 노드 활동을 실시간으로 확인하세요.
start-web-code = WebAssembly 런타임
    Auto Wi-Fi + USB Auto
    TypeScript 예제
start-web-target = 플레이그라운드 열기

start-rust-headline = Rust 앱에 임베드하고 싶어요
start-rust-body = 바로 쓸 수 있는 완전한 RNS runtime, 또는 직접 runtime을 둘러 만들 수 있는 순수 코어.
start-rust-target = prnsd or personal-rns

# 플랫폼 ("Runs on") — hero marquee label + CTA, and the dedicated page
landing-platforms-label = Runs on
landing-platforms-cta = See all →
platforms-title = Where Prns runs
platforms-lead = One engine, many homes. This quick view separates runtime platform support from specific Hopspot board support.
platforms-board-support-link = View board support & bring-up →

# Flash a Hopspot page
flash-back = Platforms
flash-back-boards = Boards
flash-card-action = Flash

# 벤치마크 페이지
benchmarks-kicker = 성능
benchmarks-title = 공개 벤치마크
benchmarks-lead = 우리는 성능을 형용사가 아니라 숫자로 다룹니다. 여기의 모든 수치는 repo 안의 결정적 harness에서 나오며, 실제 하드웨어에서 측정하고 비교가 공정한 곳에서는 RNS 레퍼런스와 대조했습니다. 수치는 suite가 안정화되는 동안 채워지고 있습니다. 아래에는 그 수치들이 따르는 방법론이 있습니다.

# 라이선스 신호 (푸터)
footer-license = 오픈 소스. MIT / Apache 2.0.
footer-trademarks = 제3자 로고와 상표는 각 소유자에게 속합니다. 이는 플랫폼, 하드웨어, 호환성 대상을 식별하기 위해서만 표시됩니다. 보증이나 승인을 주장하거나 암시하지 않습니다.

# 기여 페이지
contributing-kicker = 기준선
contributing-title = 기여
contributing-lead = 기여하는 방법 — 우리가 중요하게 여기는 것, 코드가 따르는 관례, 모든 변경이 통과해야 하는 기준입니다. 사람 기여자와 자동화된 기여자 모두에게 적용됩니다.

# 404
not-found-title = 아직 여기는 비어 있습니다.
not-found-cta = 홈으로 돌아가기
