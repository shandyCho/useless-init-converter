# 변경 이력

[English](CHANGELOG-us.md)

버전은 본체 패키지의 버전입니다. 독립 도구는 버전이 따로 있습니다.
릴리스 노트: [packages/release-notes](packages/release-notes).

## Unreleased

도구 내부에 규칙 5(유닉스 철학)를 적용합니다. `work_history.md` 246부부터.

### 삭제
- **`initconvertd start|stop|restart UNIT`**: 데몬 바이너리는 이제 데몬 역할만
  합니다. 제품 안에서 이 명령을 부르는 곳이 없었습니다. `systemctl`이나 `s6-rc`를
  쓰세요.

### 변경
- **`initconvert log UNIT`**은 `initconvert` 자체의 명령입니다(`journalctl -u`나
  `tail`을 실행합니다). 더는 `initconvertd log`가 아닙니다.
- **종료:** 요청 시 인스턴스는 `initconvertd stop-instances`가 아니라 `dbus-send`
  호출 하나(`initconvert rc stop-instances`)로 멈춥니다.
- **생성되는 s6 스크립트가 execline입니다**(`run`, `finish`, 로거). 더는
  `/bin/sh`가 아닙니다. `coredump` 서비스의 시작도 셸 스크립트에서
  `initconvert svc coredump`로 옮겼습니다. 서비스를 시작할 때 셸이 실행되지 않습니다.
- **시작 제한**(`StartLimitBurst=`/`StartLimitIntervalSec=`)은 러너가 따로 두던
  목록 대신 s6-supervise의 death tally를 셉니다. 간격 안의 모든 종료를 세며,
  수동 정지와 정상 종료도 포함됩니다. systemd가 모든 시작을 세는 것과 같습니다.
  `systemctl reset-failed`가 tally를 지웁니다.

## 0.4.0 (2026-11-11)

0.3.0 이후의 변경. 자세한 내용은 `work_history.md` 200-240부에 있습니다.

### 삭제
- **s6 사용자 매니저**(`uinitconverter-user`, `initconvertd user-manager`,
  `initconvert-user-setup`): 두 부팅 모두 `user@UID.service`는 `systemd --user`를 실행합니다.
  `systemd --user`를 다시 구현한 것이라 systemd와 GNOME의 변경을 따라갈 수 없었습니다. docs/etc.md의
  "범위"를 참고하세요. 업데이트하면 이 패키지가 제거되고(`uinitconverterd`가 Obsoletes),
  `initconvert-user-setup on`이 썼던 `user@` drop-in도 함께 지워집니다. `USER_MANAGER=`,
  `USER_EXCLUDE=`, `USER_FALLBACK_ON=`과 `uinitconverter.user_manager=`는 무시됩니다.
- **호출하는 프로그램이 없는 D-Bus 메서드:** `Manager.AbandonScope`, `GetJob`, `UnrefUnit`,
  `Unit` 인터페이스의 `TryRestart`, `ReloadOrRestart`, `ReloadOrTryRestart`(Fedora의 `/usr`에서
  이 이름을 가진 파일이 없습니다. `systemctl`은 그대로 남는 `Manager`의 `…Unit` 메서드를 씁니다).

### 변경
- **가능하면 initramfs에서도 systemd 없음(규칙 2-4):** 기본값은 `--boot-only-s6=auto`입니다. 루트가
  uinitconverter의 initramfs가 못 하는 것(LVM, MD, multipath, 네트워크 루트, 별도 `/usr`, `resume=`,
  TPM2/FIDO2/PKCS#11 LUKS 키, `mdevd` 없음)을 요구하지 않으면 s6 항목은 그 initramfs로 부팅하고, Fedora의 것을
  쓸 때는 `initconvert init`이 이유를 알려 줍니다. 명시적인 `--boot-only-s6=no`는 유지됩니다
  (`/etc/uinitconverter/boot-only-s6.off`).
- **s6 initramfs를 Rust로(규칙 6):** `mkinitramfs`는 `initconvert`의 링크이며, 이미지 안의 `rootfs`
  서비스(LUKS, 루트 마운트)와 s6-svscan의 finish(`switch_root`)는 `initconvert initramfs root|switch`입니다.
  이미지에 `initconvert`가 들어가고, bash는 복구 셸용으로만 남습니다. `UINITCONVERTER_INITRAMFS_SOURCE`로
  이미지의 트리를 옮길 수 있습니다(테스트용).
- **kernel-install 플러그인을 Rust로(규칙 6):** `90-uinitconverter.install`은 `initconvert`의 링크이며,
  셸 플러그인처럼 s6 부팅 항목을 유지합니다(`ls -v`와 같은 순서로 정렬).
- **2단계 부팅, 종료, init-selinux를 Rust로(규칙 6):** s6-linux-init의 `rc.init`, `rc.shutdown`,
  `rc.shutdown.final`, `runlevel`은 execline 한 줄이며, 일은 `initconvert rc init|shutdown|shutdown-final`이
  하고 `/usr/sbin/init-selinux`는 `initconvert`의 링크입니다. `initconvert init`(s6-linux-init를 다시 만듦)
  뒤에 적용됩니다.
- **직접 작성한 s6 서비스를 Rust로 시작합니다(규칙 6):** `/etc/uinitconverter/s6`의 각 `run`(dbus,
  syslog, udevd, earlyoom, journal-relay, fd 저장소, linkwatch, initconvertd)은 execline 한 줄입니다.
  셸이 하던 일(버스 선택과 dbus-broker 대체, syslog 선택, udevd 샌드박스 대체, earlyoom 인자)은
  `initconvert svc NAME`이 하고 프로그램을 exec합니다.
- **systemd-journald가 기본으로 꺼집니다(규칙 2, 핵심 결정 5를 뒤집음):** 새 기본값은 `JOURNAL=no`로,
  rsyslog(또는 syslog-ng, 이제 필수)와 `/var/log/uinitconverter` 아래 서비스별 s6-log 디렉터리를 씁니다.
  이때 `journalctl -u`와 `systemctl status`의 로그 줄은 비어 있습니다(`initconvert init --journal=yes`이면
  journald 실행). 업데이트는 기존 설치의 journald를 유지합니다(설정이 없으면 `JOURNAL=yes`를 적음).
  `initconvert init`은 `JOURNAL=`을 어느 값이든 적으므로, 이후의 업데이트가 `--journal=no`를 되돌리지 않습니다.
- **systemd-resolved가 기본으로 꺼집니다(규칙 2):** 새 기본값은 `RESOLVED=no`이며, s6 부팅에서는
  NetworkManager가 DNS를 처리합니다(`initconvert init --resolved=yes`이면 resolved 실행). 업데이트는 기존
  설치를 그대로 둡니다. `uinitconverter.conf`에 설정이 없으면 패키지가 `RESOLVED=yes`를 적습니다.
  `initconvert init`은 `RESOLVED=`를 어느 값이든 적으므로, 이후의 업데이트가 `--resolved=no`를 되돌리지 않습니다.
- **systemd 프로그램을 더 대체했습니다(규칙 2):** systemd-vconsole-setup은 kbd의 `loadkeys`/`setfont`
  (`vconsole` s6 oneshot)로, systemd-backlight@는 `backlight` s6 oneshot(같은 파일
  `/var/lib/systemd/backlight`)으로, systemd-user-sessions는 tmpfiles 단계와 `rc.shutdown`
  (`/run/nologin`)으로, systemd-machine-id-setup은 dbus의 `dbus-uuidgen`으로 바꿨습니다. 세 유닛은 기본
  `INCLUDE`에서 빠졌고, `systemd-user-sessions.service`는 `tmpfiles-setup`에, `systemd-vconsole-setup.service`는
  `vconsole`에 연결됩니다(`DEP_MAP`). 새 서비스는 `uinitconverter-sysinit`에 들어가거나 의존 대상이 되지
  않습니다. reload의 dry run이 없는 버전에서 업데이트할 때 dbus나 gdm이 재시작되기 때문입니다.
- 초기 부팅 단계(`/usr/libexec/uinitconverter/early`)를 셸 스크립트 대신 `initconvert`의 일부(그 링크)로
  옮겼습니다(규칙 6).
- **서비스는 자기 데이터베이스의 유닛을 실행합니다.** gen이 만드는 s6-rc 데이터베이스에 각 유닛을 쓰고
  (longrun은 `data/unit`, 나머지와 인덱스는 실행되지 않는 `uinitconverter-units` 서비스), 러너와 타이머가
  거기서 읽습니다. systemd가 `daemon-reload` 전까지 읽어 둔 유닛을 실행하는 것과 같아서,
  `systemctl daemon-reload` 없이 고친 유닛 파일은 이제 다음 시작에 영향을 주지 않습니다. 패키지 업데이트는
  영향이 없습니다(Fedora의 RPM 트리거가 reload합니다). 되돌린 부팅도 이제 새 데이터베이스가 아니라 부팅한
  데이터베이스의 인덱스를 읽습니다.
- `NeedDaemonReload`를 계산합니다(전에는 항상 false). 마지막 reload 뒤에 유닛 파일이 바뀌면
  `systemctl status`가 경고합니다.
- `tests/vm-user-manager.sh`는 GNOME 아래의 `systemd --user`(세션, 로그인 화면, 앱, 수명 주기,
  제한된 사용자)를 systemd 부팅에서, `BOOT=s6`이면 s6 부팅에서 확인합니다. `tests/vm-s6-boot.sh`의
  모드는 `default`, `old-config`, `upgrade`(s6 사용자 매니저를 켠 0.3.0에서 s6 부팅 중에 README대로
  업데이트), `rollback`(`uinitconverter.db=previous`)입니다.
- 유닛 파일은 있지만 실행 중인 s6 데이터베이스에 없는 유닛을 시작하면 이유를 알려 줍니다. 다음 부팅용
  데이터베이스에만 있거나(미뤄진 reload, 아래 참고), `systemctl daemon-reload`가 필요합니다(새 유닛 파일의
  버스 활성화는 reload하지 않음). 전에는 "not translated to s6-rc"라고 했습니다.
- 초기 부팅의 rfkill 복원이 장치마다 로그를 남깁니다(복원, 저장, `ID_PATH`가 없어 건너뜀).

### 수정
- **0.3.0에서 업데이트한 뒤 로그인이 막히던 문제.** 0.3.0의 데이터베이스는 `systemd-user-sessions.service`를
  실행했습니다. 업데이트가 이를 빼므로(규칙 2) s6-rc-update가 멈추고, 그 `ExecStop=`이 `/run/nologin`을 써서
  다음 부팅까지 root가 아닌 모든 로그인(로그아웃 뒤 GDM 로그인, SSH)을 거부했습니다. 이제 그런 업데이트 뒤에는
  시스템이 종료 중이 아니면 gen이 `/run/nologin`을 다시 지웁니다.
- **s6 부팅에서 업데이트하면 시스템 매니저가 멈출 수 있었습니다.** 패키지가 `syslog`와 `journal-relay`를
  `uinitconverter-sysinit` 번들의 멤버로 넣어 두었고, `initconvert init`이 이를 지웠습니다(journald
  설치). 이 디렉터리는 `%config`라서 RPM 업데이트 때마다 둘이 되살아났고, dbus와 initconvertd가 의존하는
  번들에 내려가 있는 서비스로 들어갔습니다. 그 상태에서 `initconvert init` 전에 reload가 일어나면(같은
  트랜잭션의 다른 패키지가 부르는 `systemctl daemon-reload`) s6-rc-update가 dbus, 그것을 쓰는 모든
  서비스, initconvertd 자신을 재시작하려 했고, initconvertd는 자기 자신이 멈추기를 기다렸습니다.
  `systemctl`도 새 SSH 로그인도 되지 않았습니다. 이제 패키지는 이 멤버들을 넣지 않고, `--journal=no`일 때
  `initconvert init`이 추가합니다(`resolv`와 같은 방식). 이 수정이 있으면 0.3.0에서의 업데이트도
  안전합니다(멤버가 되살아나지 않고, `--journal=no` 설치에서는 RPM이 지운 뒤 init이 다시 추가합니다).
- **reload가 실행 중인 서비스를 재시작하지 않습니다.** 새 데이터베이스를 라이브로 설치하면 실행 중인
  서비스가 하나라도 재시작되는 경우(`s6-rc-update -n`으로 확인) 다음 부팅용으로만 설치하고 그렇다고
  알립니다("not applied to the running system: the update would restart ..."). systemd의 daemon-reload도
  아무것도 재시작하지 않습니다. s6-rc-update는 새 서비스에 대한 의존성이 생긴 서비스를 모두 재시작합니다.
  journald를 켜자 logind와 gdm을 포함해 거의 모든 서비스가 재시작되었고, dbus를 재시작하는 reload는 위와
  같이 그 업데이트를 기다리던 initconvertd를 멈췄습니다. 아무것도 재시작하지 않는 reload(새 유닛, 바뀐
  명령줄)는 그대로 라이브로 적용됩니다.
- **reload 중의 D-Bus 활성화가 더 이상 이전 데이터베이스에서 서비스를 시작하지 않습니다.** initconvertd가
  업데이트 옆에서(`s6-svc -u`) 시작했고, s6-rc-update의 전환이 다시 끝냈습니다. 이제 reload를 기다렸다가
  새 데이터베이스에서 시작합니다.

## 0.3.0 (2026-11-09)

0.2.2 이후의 변경. 단계별 자세한 내용은 `work_history.md` 130-199부에 있습니다.

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
- s6 부팅에서는 이제 s6 사용자 매니저가 기본값입니다. `USER_MANAGER=`가 없으면 `uinitconverter-user`가
  설치되어 있을 때 `user@UID`가 `initconvert-user-manager`를 실행하며, `uinitconverter`가 이 패키지를
  Recommends합니다(이전에는 Suggests). `USER_MANAGER=systemd`는 `systemd --user`를 유지합니다. systemd
  부팅은 그대로입니다(거기서는 `initconvert-user-setup on`으로 켭니다).
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
- 동적 유닛(s6 부팅의 `user@UID`, 사용자 매니저의 모든 유닛)의 시작이, 서비스는 계속 시작되는데 몇 밀리초 만에
  `failed`로 끝나지 않습니다. `s6-svc -u` 뒤 s6-supervise가 명령을 받아들이기 전에 상태를 읽어, 상태가 아직
  "멈춤, 시작 원하지 않음"이었습니다. 부팅 때 이 때문에 logind의 `user@1000` 시작 작업이 일찍 끝나 GDM이 사용자
  매니저와 그 버스보다 먼저 GNOME 세션을 시작했고, 세션이 실패했습니다(VM에서 일부 재부팅 뒤 세션이
  없었습니다). 이제 `initconvertd`가 상태에 명령이 반영될 때까지 기다립니다.
- 주 프로세스가 cgroup에 도우미(ibus-daemon의, gvfsd의)를 남기고 끝날 때 서비스의 `finish`가 0.1초 걸리지
  않습니다. 러너가 이들에 시그널을 보낸 뒤 100ms 뒤에야 다시 확인했는데, 이제 5ms 뒤부터 두 배씩 100ms까지
  늘려 확인하며 한도는 그대로 2초입니다. VM의 재부팅에서 tester 유닛의 `finish`가 50ms를 넘은 적이 없습니다
  (이전: 재부팅마다 1-2번, 0.10-0.12초).
- 동적 유닛(사용자 매니저의 모든 유닛)을 멈출 때 `s6-svc -wD`와 `s6-svunlink`(그리고 이들의 `s6-svlisten`,
  `s6-ftrigrd`, 정지마다 프로세스 다섯 개쯤)를 실행하지 않습니다. `initconvertd`가 s6-supervise와 s6-svscan의
  제어 FIFO에 직접 쓰고 상태 파일과 감독자의 잠금을 읽으며, 그것들이 없으면 명령으로 돌아갑니다. VM에서
  재부팅 때 tester의 사용자 매니저가 0.99-1.05초 대신 0.81-0.93초에 멈췄고, `s6-svunlink`의 5초 대기에
  걸리지 않았습니다.
- `initconvertd`와 러너가 `s6-svstat`을 실행하지 않고(정지마다 여러 번, 종료 때 초당 100번쯤) 서비스의
  `supervise/status`에서 상태를 읽습니다. 그만큼 CPU가 남아, VM에서 GNOME 재부팅 때 tester의 사용자 매니저가
  10번 모두 0.55-0.68초에 멈춥니다(이전: 10번 중 6번 0.64-0.80초, 4번은 로그아웃이 멈춘 유닛이 D-Bus로 다시
  활성화되며 3-5.6초 걸림).
- s6 사용자 매니저가 GNOME 로그인 때 `graphical-session-pre.target`을 불필요하다며 멈추지 않습니다.
  (`StopWhenUnneeded=`를 이 타깃의 시작과, 이를 요구하는 `gnome-session-pre.target`의 시작 사이에
  확인했습니다.) GNOME의 세션 리더는 로그아웃 때 이 타깃을 기다리는데, 이미 멈춰 있어 곧바로 끝났고,
  매니저는 gnome-session이 끝나기 전에 종료 요청을 받았으며, gnome-session은 이미 없는 클라이언트를
  1-2초 기다렸습니다. 이제 시작 작업이 진행 중이면 확인을 미룹니다.
- 종료하는 사용자 매니저가 스코프(자동 시작되거나 실행된 앱)를 systemd의 종료 트랜잭션처럼 유닛과 함께
  멈춥니다. 이전에는 마지막에 멈춰서 gnome-session이 그 안의 자동 시작 앱을 기다렸습니다.
- 로그아웃이나 재부팅 때 사용자 매니저가 첫 시그널을 0.5초쯤 일찍 보냅니다. 함께 멈출 유닛을 알려진 유닛을
  한 번 읽어 찾고(이전에는 유닛과 의존성마다 모든 유닛의 `.requires` 디렉터리를 다시 읽었습니다), 각 유닛은
  가장 느린 유닛을 기다리는 단계 단위가 아니라 그 뒤에 오는 유닛들이 멈추는 대로 바로 멈춥니다. VM에서
  재부팅 때 tester의 매니저가 0.7초쯤에 멈춥니다(`systemd --user` 0.65초).
- `GetAll` 응답이 속성을 systemd의 순서로 보냅니다(이전에는 해시 순서). 그래서 `systemctl show -p A,B`가
  systemd에서처럼 출력하고, `--value`로 여러 값을 읽는 스크립트도 같은 순서를 받습니다.
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
