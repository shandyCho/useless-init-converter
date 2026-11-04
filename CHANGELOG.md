# 변경 이력

[English](CHANGELOG-us.md)

버전은 본체 패키지의 버전입니다. 독립 도구는 버전이 따로 있습니다.
릴리스 노트: [packages/release-notes](packages/release-notes).

## 미출시

0.2.2 이후의 변경. 단계별 자세한 내용은 `work_history.md` 130-157부에 있습니다.

### 추가
- 작업 모드 `fail`, `replace-irreversibly`, `flush`, `ignore-dependencies`, `ignore-requirements`
  (이전에는 `replace`로 실행), 그리고 `replace`도 이제 유닛에서 아직 기다리는 충돌 작업을 취소합니다.
  `OnFailureJobMode=`와 `OnSuccessJobMode=`(GNOME은 종료 타깃을 `replace-irreversibly`로 시작합니다).
  알 수 없는 모드는 거부합니다. 제한은 docs/etc에 있습니다.
- `Accept=yes` 소켓: 유닉스 경로는 `s6-ipcserverd`로, TCP 포트는 `s6-tcpserverd`로(선택 패키지
  s6-networking, 예: `sshd.socket`). 연결마다 systemd와 같은 이름의 템플릿 인스턴스가 실행되고, 실행되는
  동안 유닛으로 목록에 나오고 상태를 보고 멈출 수 있습니다.
- `GetUnitByPIDFD`(polkitd가 호출자마다 묻습니다), 그리고 작업 대상 유닛만이 아니라 모든 상태 변화에 대한
  `PropertiesChanged`.
- **s6 사용자 매니저(선택).** `uinitconverter.conf`의 `USER_MANAGER=s6`이나 커널 명령줄의
  `uinitconverter.user_manager=s6`이 있으면 `user@UID.service`에 `systemd --user` 대신
  `initconvert-user-manager`(`uinitconverter-user` 패키지)를 실행합니다. 기본값은 계속 `systemd`입니다. 테스트 VM에서 GDM 그리터와
  GNOME 세션을 수정 없이 실행합니다. 로그아웃, 다시 로그인, 사용자 전환, linger 사용자, RPM 업데이트
  도우미, 재부팅, 타이머와 path 유닛, 소켓(`Accept=yes` 포함), systemd-oomd 보고, podman quadlet 유닛,
  flatpak 앱이 동작합니다. 트랜잭션의 유닛은 병렬로 시작합니다. 사용자를 제대로 받지 못하면(GNOME의 실패
  경로, 충돌) 그 부팅 동안은 그 자리에서 `systemd --user`로 넘깁니다(`USER_FALLBACK_ON=`).
  [s6 사용자 매니저](docs/etc.md#s6-사용자-매니저-진행-중) 참고.
- **일반 systemd 부팅에서 s6 사용자 매니저**(선택): `sudo initconvert-user-setup on`(패키지
  `uinitconverter-user`. 이제 s6 부팅 패키지가 필요 없습니다)이 모든 `user@`에 이 매니저를 씁니다.
  `off`는 `systemd --user`로 되돌리고, `status`는 사용자마다 무엇이 실행되는지 보여 줍니다. 그 부팅에서도
  커널 플래그 `uinitconverter.user_manager=s6`이 더 이상 필요 없고, `uinitconverter.user_manager=systemd`는
  여전히 한 부팅 동안 끕니다.
- 데몬 환경의 `UINITCONVERTER_LOG_JOBS=1`은 모든 작업 요청을 호출자의 프로세스 이름과 함께 기록합니다.
- `packages/build-rpms.sh --test`(`--shared-target`, `--nocheck`): 테스트용 빠른 RPM 빌드.
  [빌드](packages/README.md#빌드) 참고.
- `tests/vm-s6-boot.sh`: GNOME 테스트 VM에서 RPM, `initconvert init`, s6 부팅을 확인합니다.
  `BOOT=s6`이면 `tests/vm-user-manager.sh`의 session·lifecycle 확인을 s6 부팅에서 실행합니다.
- 도구들이 함께 쓰는 형식(`result` 값, 러너의 명령, 독립 fd 저장소와의 프로토콜, 설정 키, 색인)의
  테스트. [도구 사이의 약속](docs/etc.md#도구-사이의-약속) 참고.

### 변경
- `uinitconverterd`는 더 이상 s6-rc와 dbus-daemon을 요구하지 않습니다. s6 부팅에 필요한
  `uinitconverter` 패키지가 여전히 요구합니다.
- **명령의 이름이 `initconvert*`입니다**: `initconvert`(이전 `uinitconverter`), `initconvertd`,
  `initconvert-gen`, `initconvert-runner`, `initconvert-timers`, `initconvert-user-manager`, 그리고
  독립 도구 `initconvert-fdstore`, `-notify`, `-journal-relay`, `-linkwatch`. 패키지, 경로, s6 서비스,
  SELinux 모듈, D-Bus 인터페이스, 커널 옵션, 변수는 `uinitconverter` 이름을 유지합니다. 설치된 시스템의
  이전: [initconvert 명령으로 이전하기](packages/README.md#initconvert-명령으로-이전하기).
- systemd와 같은 작업 부수 효과: 시작하는 유닛을 `Conflicts=`에 적은 유닛을 멈추고, 유닛을 멈추면
  `PartOf=`, `BindsTo=`, `Requires=`로 그 유닛을 가리키는 실행 중 유닛도 멈추며, 재시작하면 `PartOf=`인
  유닛도 재시작합니다. `StopWhenUnneeded=`를 따르고, 타깃의 상태는 타깃 자신의 것이며, `STOPPING=1`을
  보낸 `Type=notify` 서비스는 멈춥니다.
- 어떤 유닛의 시작이 대기 중일 때 같은 유닛을 또 시작하면 systemd처럼 그 작업에 합쳐집니다.
- `NotifyAccess=all`인 `Type=notify` 서비스는 `MAINPID=`로 주 프로세스를 넘길 수 있고(podman의 quadlet
  유닛), 러너는 알려진 프로세스를 따라갑니다.
- `AttachProcessesToUnit`은 권한 없는 호출자에 대해 systemd의 규칙을 따르고, 컨트롤러 목록을 적은
  `Delegate=`도 위임으로 봅니다.
- 새 패키지 `uinitconverter-user`(noarch): s6 사용자 매니저의 진입점 `initconvert-user-manager`
  (`initconvertd`로의 링크)와 `/run/uinitconverter-user`(대체 기록)용 tmpfiles.d 항목. 이 패키지가
  없으면 `USER_MANAGER=`와 상관없이 `user@`는 `systemd --user`를 씁니다.

### 수정
- s6 부팅에서 SELinux로 제한된 사용자(`user_u`, `staff_u`)의 s6 사용자 매니저: 러너에 보내는 READY=1이
  거부되어(제한된 도메인은 `init_t`의 데이터그램 소켓에 보낼 수 없습니다) `user@`가 90초 뒤 시간 초과로
  실패했습니다. 이제 러너의 notify 소켓은 자체 타입 `uinitconverter_notify_t`를 쓰고,
  `/run/uinitconverter-user`는 `/run/user/UID`처럼 레이블됩니다.
- 재시작할 때 그 유닛에 `BindsTo=`를 둔 실행 중인 유닛도 systemd의 재시작 작업처럼 다시 시작합니다.
  pipewire를 재시작하면(RPM 업데이트 도우미가 합니다) wireplumber가 멈춘 채로 남아, 다음 로그인까지 소리가
  나지 않았습니다.
- 소켓 활성화가 이제 systemd의 시작 작업처럼 서비스의 `Requires=`, `BindsTo=`, `Wants=`(`.wants/`,
  `.requires/` 포함)도 시작합니다. s6 사용자 매니저에서 wireplumber(`pipewire.service.wants/`)가 시작되지
  않아 소리가 나지 않았고, gnome-shell은 종료할 때 재생되지 않은 소리를 기다리느라 로그아웃과 재부팅 때마다
  `TimeoutStopSec=`(5초)를 기다렸습니다. `Wants=`, `Requires=`, `Upholds=` 속성도 이 디렉터리의 항목을
  보여 줍니다.
- `KillMode=control-group`(기본값): 이제 정지할 때 systemd처럼 유닛의 나머지 프로세스에도 주 프로세스와
  함께 시그널을 보냅니다. 이전에는 주 프로세스가 끝난 뒤에야 보냈습니다(gnome-shell은 SIGTERM을 받으면
  이미지 로더를 기다립니다).
- 버스 이름을 얻기 전에 프로세스가 0으로 끝나는 `Type=dbus` 서비스(로그인 때의
  evolution-alarm-notify)와, 시작이 확인되기 전에 0으로 끝나는 `Type=simple`/`exec` 서비스가 이제
  systemd처럼 결과 `success`로 `inactive (dead)`가 되고 시작 작업도 완료됩니다. 이전에는 작업이
  실패했고 `Type=dbus` 유닛은 `failed`(`exit-code`)로 남았습니다.
- `JOURNAL=no`에서 로그 디렉터리를 더는 만들 수 없는 사용자 매니저의 로거는 매니저의 런타임
  디렉터리 아래에 대신 기록하고, 그렇다고 한 번 알립니다. GDM은 로그인할 때 그리터의 홈을 지우는데,
  그리터의 매니저는 그 뒤로도 10초쯤 실행됩니다. 그 사이 다시 시작한 세션 버스의 로거가 매초 실패했고
  (`mkdir: cannot create directory '/run/gdm/home/gdm-greeter'`), 버스의 출력은 파이프에 남았습니다.
  부팅의 로거는 계속 다시 시도합니다.
- s6 사용자 매니저가 `/org/freedesktop/systemd1/unit/self`(호출한 프로세스 자신의 유닛)에 응답하고,
  거기서 `GetUnitByPID`가 사용자 유닛을 알려 줍니다(이전에는 모든 프로세스에 `user@UID.service`).
  `systemd-run --user --scope`가 "Failed to request invocation ID for unit: Unknown object"로
  실패해서, Ptyxis가 새 터미널마다 이 줄을 출력하고 셸을 스코프 없이 시작했습니다.
- s6 부팅에서 `initconvert-user-setup status`가 s6 사용자 매니저를 알려 줍니다. 이전에는 그곳에서
  `user@`의 첫 프로세스인 `initconvert-runner`를 출력했습니다. "fell back" 표시도 매니저처럼 사용자
  자신이 소유한 기록만 따지고, 사용자의 런타임 디렉터리에 있는 기록도 읽습니다.
- s6 부팅에서 s6 사용자 매니저가 끝날 때 자신의 `initconvert-runner`(부모 프로세스, PAM 세션을 가짐)와
  러너의 notify 헬퍼에 더 이상 시그널을 보내지 않습니다. 이전에는 root의 `user@0`(`sudo`로 시작)이
  실패(`Result=signal`, SIGKILL)로 끝나고 PAM 세션이 닫히지 않았으며, 다른 사용자의 정지도 매번
  불필요하게 1초를 기다렸습니다.
- 종료할 때 사용자들의 `user@`와 `user-runtime-dir@`가 systemd처럼 동시에 멈춥니다. 이전에는 사용자마다
  차례로 멈춰서, s6 사용자 매니저로는 사용자 셋에 8-11초가 걸렸습니다.
- `Accept=yes` 인스턴스의 `LISTEN_FDNAMES=`는 소켓의 `FileDescriptorName=`을 따릅니다(이전에는 항상
  `connection`). 그래서 systemd의 varlink 서비스가 응답합니다.
- 소켓의 `Symlinks=`가 링크의 상위 디렉터리를 만듭니다.
- `/var`(또는 `/var/lib`)가 Fedora Cloud의 btrfs `var` 서브볼륨처럼 별도 파일 시스템이어도 s6 부팅이
  됩니다. 2단계가 데이터베이스를 읽기 전에 fstab에 따라 마운트합니다. 이전에는
  `s6-rc-init: fatal: unable to open /var/lib/uinitconverter/compiled`로 2단계에서 멈췄습니다.
- `/boot`가 파티션이 아니라 btrfs 서브볼륨이어도(grubby가 커널을 `/boot/boot/vmlinuz-…`로 알려 줌)
  `initconvert init --default-s6`이 s6 항목을 기본값으로 만듭니다. 이전에는 항목은 추가했지만
  `no s6 entry for …`를 출력하고 기본값을 바꾸지 않았습니다.

### 알려진 문제
- s6 부팅의 시스템 매니저 아래에서는 s6 사용자 매니저를 테스트 VM에서만(`tests/vm-s6-boot.sh`) 실행했고,
  실제 하드웨어에서는 아직입니다.
- `uinitconverter-user`만 있는(SELinux 모듈이 없는) systemd 부팅에서는 제한된 사용자의 매니저가
  `/run/uinitconverter-user`에 기록을 쓰지 못합니다(시작할 때마다 거부 하나). 대신 런타임 디렉터리에 쓰며,
  `initconvert-user-setup status`는 그곳도 읽습니다.

## 0.2.2 (2026-11-02)

### 수정
- s6 부팅에서 `selinux-policy-targeted`를 자체 SELinux 모듈을 가진 패키지(`passt-selinux` 등)와 함께
  업데이트해도 실행 중인 시스템이 망가지지 않습니다. 이 업데이트가 `/run/service` 아래의 레이블을
  복원하면서 s6-svscan과 s6-linux-init 서비스의 fifo에 `init_t`가
  쓸 수 없게 되었고, 그 뒤의 모든 재적재가 `unable to manage new service directories ... Permission denied`로
  실패했습니다. 이제 SELinux 모듈이 이 파일들의 레이블을 따로 지정하고, 패키지 업데이트와
  `uinitconverter init`이 이전 업데이트가 남긴 상태를 복구합니다.
- 재적재의 마지막 시도가 실패해도 이전 시도들처럼 실행 중인 서비스의 스캔 디렉터리 링크를 되돌립니다.
- 실행되기 전에 시작이 실패한 임시 서비스를 내리고 저장해 둔 표준 입출력을 놓습니다. 이전에는 호출자의
  파이프를 계속 잡고 있어, `systemctl --user -M`을 실행한 스크립틀릿에서 `dnf`가 끝없이 기다렸습니다.

## 0.2.1 (2026-11-02)

### 수정
- `PrivateNetwork=`는 이제 systemd처럼, 꺼 두지 않는 한 `PrivateMounts=`를 함께 적용하고 새 네트워크
  네임스페이스에 맞춰 `/sys`를 다시 마운트합니다. 이전에는 이런 서비스가 호스트의 마운트 네임스페이스에
  남았고 `/sys/class/net`에 호스트의 장치가 보였습니다. 다시 마운트하지 못하면 서비스는 그대로 시작하고
  그 사실을 로그에 남깁니다.
- 종료할 때 디스플레이 관리자(`display-manager.service`)도 세션 스코프와 함께 멈추도록 요청합니다.
  systemd도 둘을 함께 멈춥니다. 이전에는 gdm이 세션 종료를 보고 새 그리터를 최대 여섯 번 시작하려다
  모두 거부되어, 종료할 때마다 `gnome-session-init-worker` 코어 파일이 여섯 개 남았습니다.

### 변경
- `.path` 유닛의 inotify와 fd 저장소의 소켓 처리를 libc 직접 호출에서 rustix로 옮겼습니다.
  의도한 동작 변경은 없습니다. 수신 자격 증명(`SCM_CREDENTIALS`, `SO_PEERCRED`)은 libc에
  남겼습니다. 커널이 pid 0을 보고할 수 있는데, rustix의 `UCred`는 이 값을 담을 수 없습니다.
- s6 initramfs는 암호화된 볼륨이 없으면 `cryptsetup`이 필요하지 않고, `dm_crypt`도 LUKS를 쓸 때만
  불러옵니다. `/etc/crypttab`이나 명령줄에 암호화 볼륨이 지정되어 있는데 cryptsetup이 없으면 빌드가 실패합니다.
- `uinitconverter init --boot-only-s6=yes`는 LVM이나 MD 위의 루트를 경고합니다. 이 initramfs는 이를 조립할 수 없습니다.
- 종료할 때 uinitconverterd가 세션 스코프를 기다린 시간을 로그에 남깁니다.
- README는 첫 설치만 다루고, 나머지는 [docs/etc.md](docs/etc.md)로 옮겼습니다.

## 0.2.0 (2026-11-01)

첫 공개 릴리스(태그 `initial`, 2fac273) 이후의 변경 사항입니다.

### 추가
- s6 부팅의 zram 스왑: zram-generator를 실행하는 oneshot.
- 부팅 시 요청 기반 소켓 활성화.
- `BUS=daemon|broker`: 시스템 버스 선택(dbus-daemon 또는 dbus-broker).
- `SYSLOG=rsyslog|syslog-ng`: s6 부팅의 syslog 데몬 선택.
- `INIT=`는 init 백엔드를, `TOOLS=`는 다른 패키지의 프로그램을 지정합니다.
- s6 서비스 `coredump`: systemd-coredump의 리스너로 코어 덤프를 보관합니다.
- `tests/hardencompare.sh`, `tests/memtest.sh`.

### 변경
- rsyslog, earlyoom, udevd, 시스템 버스를 유닛의 샌드박스 안에서 실행합니다.
- `PrivateUsers=`를 구현했습니다(colord, upower 등 이 설정을 쓰는 서비스가 자체 사용자 네임스페이스를 갖습니다).
- 읽기 전용 마운트를 systemd와 맞췄습니다.
- `/var/log/uinitconverter`는 0750 root:wheel입니다.
- systemd와 같이 하드 `RLIMIT_NOFILE`은 524288, 소프트 `RLIMIT_CORE`는 unlimited입니다.
- Type=notify 도우미는 fork로 남지 않고 다시 exec합니다.
- `OnCalendar=`의 시간대는 `TZ` 전환 대신 jiff를 쓰므로, `uinitconverterd`가
  이름이 지정된 시간대의 다음 실행 시각을 직접 계산합니다.
- 소스: rustfmt 적용, 큰 파일을 역할별로 분리. 로직 변경 없음.

### 수정
- 종료 시 systemd처럼 세션 scope를 `user@UID`보다 먼저 멈춥니다. gnome-shell의
  종료 시 충돌을 해결하기 위한 수정이며, 알려진 문제를 참고하세요.
- DST 변경 전후의 `OnCalendar=`가 systemd와 같아졌습니다. `minutely`가 반복되는
  시간의 두 번째 구간을 건너뛰지 않고, 반복되는 시각이 두 번 실행되지 않으며,
  시간대에서 건너뛴 날짜가 다른 날로 옮겨지지 않습니다.
- 소프트 `RLIMIT_CORE`가 kernel_t에서 init_t로 전환될 때 유지됩니다.
- rsyslog를 샌드박스에 넣기 전에 `/run/user`를 만듭니다.
- `selinux`: restorecon이 호출마다 file_contexts를 불러오고 해제합니다.
- 보안 검토에 따른 입력 검사: 임시 유닛, fd 저장소, runner. linkwatch는 자신과
  같은 사용자의 대기 프로세스에만 동작합니다.

### 제거
- `AUTO_MASK=`.

### 알려진 문제
- 종료 순서 변경으로 gnome-shell 충돌이 해결되는지는 확인되지 않았습니다.
  지금까지 확인한 정상 종료는 2회뿐이고, 로그인 화면에서의 종료는 없었습니다.
- `tests/hardencompare.sh`는 rtkit을 여전히 s6에서 더 약하다고 보고합니다(`ns_mnt`). 이 유닛은
  `PrivateNetwork=`만 설정하므로 파일 시스템 제한이 빠진 것은 아닙니다.
