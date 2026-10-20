# sdcompat

[English](README-us.md) | **한국어**

s6를 위한 systemd 호환 계층입니다. Fedora의 유닛 파일, `systemctl`, libsystemd,
D-Bus 클라이언트는 그대로 두고, 시스템은 **s6-linux-init + s6-rc**가 실행합니다.

```
 systemctl / dnf 스크립틀릿 / libsystemd 클라이언트 / dbus-daemon 활성화
              │ org.freedesktop.systemd1 (시스템 버스 + /run/systemd/private)
              ▼
   sdcompat 데몬 ── 작업(job), 유닛 상태, enable/disable/preset, scope
              │ s6-rc -u/-d, s6-svc, s6-svstat
              ▼
   s6-rc 데이터베이스  ◄── sdcompat reload: 유닛 파일 → s6-rc 소스 → 컴파일
              │
   s6-supervise ── run/finish 스크립트가 `sdcompat run|finish|up|down` 호출
                   (User=, Environment=, 소켓, NOTIFY_SOCKET, Restart= 등)
```

라이브러리를 바꿔치기하는 방식이 아니라 프로토콜 수준에서 동작합니다. 진짜
`systemctl`과 `libsystemd`가 systemd와 대화하듯 그대로 sdcompat과 대화합니다.

## 구성 요소 (바이너리 하나, `sdcompat`)

| 명령 | 역할 |
|---|---|
| `sdcompat reload` | 모든 유닛을 s6-rc 소스 트리로 변환하고, 직접 작성한 s6 서비스(`EXTRA_SOURCE`)와 함께 컴파일한 뒤 라이브 상태를 전환합니다(`s6-rc-update`). 변환 보고서를 남깁니다. |
| `sdcompat daemon` | 시스템 버스 **그리고** `/run/systemd/private`에서 `org.freedesktop.systemd1`을 제공합니다(systemd 259 이상의 `systemctl`은 root로 실행될 때 이 소켓만 사용합니다). |
| `sdcompat run/finish/up/down` | 생성된 s6 스크립트가 호출합니다. systemd가 `fork()`와 `exec()` 사이에서 하는 일을 합니다. |
| `sdcompat timer-run` | `.timer` 하나를 s6 longrun으로 실행합니다. 다음 실행 시각을 계산하고 데몬에 유닛 시작을 요청합니다. |
| `sdcompat path-run` | `.path` 유닛 하나를 s6 longrun으로 실행하며 inotify로 감시합니다. |
| `sdcompat start/stop/restart UNIT` | 데몬용 최소 클라이언트입니다(private 소켓 또는 버스). |
| `sdcompat notify` | `systemd-notify` 대체입니다. |
| `sdcompat journal-relay` | `JOURNAL=no`일 때 journald의 클라이언트 소켓(`/run/systemd/journal/socket`과 `stdout`)을 열고 받은 내용을 rsyslog로 넘깁니다. `systemd --user`의 세션 버스인 dbus-broker는 이 소켓이 없으면 시작하지 않습니다. |
| `sdcompat log UNIT [-f]` | 서비스 로그를 보여줍니다(`journalctl -u`, `JOURNAL=no`이면 s6-log 디렉터리). |
| `sdcompat enable/disable/mask/preset/is-enabled` | 오프라인 유닛 파일 관리입니다. |

## 구현된 기능

**변환** (`.service` → longrun/oneshot, `.target` → bundle, `.timer`/`.path` → longrun, `.mount`/`.swap` → oneshot)
- `Requires=`, `Requisite=`, `BindsTo=`, 그리고 `After=`/`Before=`와 함께 쓰인 `Wants=`는 s6-rc 의존성이 됩니다. 타깃은 `Wants=` 전부를 끌어옵니다.
- `.wants/`, `.requires/`, `.upholds/` 디렉터리, 별칭, 템플릿/인스턴스, 마스킹, 드롭인, 지정자(specifier).
- 암묵적 의존성: `DefaultDependencies=` → `sysinit.target`, `Type=dbus` → `dbus.socket`.
- 의존성 순환은 컴파일을 실패시키는 대신 끊고 보고합니다.
- `DEP_MAP`은 systemd 유닛을 직접 작성한 s6 서비스에 대응시킵니다. 예: `dbus.service` → `dbus`.
- 활성화된 `.socket`은 자신의 서비스를 끌어옵니다. 소켓은 러너가 바인드하며, 이름 또는 `Service=`로 짝을 찾습니다.
- `Conflicts=`: 두 유닛이 모두 부팅 시 시작될 경우, 충돌 대상은 타깃에서 빠집니다(예: `getty@tty1.service`보다 `gdm.service`). 실행 중에는 유닛을 시작하거나 재시작할 때 충돌하는 유닛을 멈춥니다.

**마운트, 스왑, path 유닛**
- `.mount`와 `.swap` 유닛은 `mount(8)`/`swapon(8)`을 실행합니다. 상태는 커널(`mountinfo`, `/proc/swaps`)에서 읽으므로, 누가 마운트했든 `systemctl status tmp.mount`가 정확합니다. `RequiresMountsFor=`는 해당 경로를 덮는 마운트 유닛에 대한 의존성이 됩니다.
- `.automount` 유닛은 부팅 시 바로 마운트됩니다(autofs는 지원하지 않습니다).
- `/etc/fstab`은 여전히 초기 부팅 서비스 `mount-fs`가 마운트하며, `local-fs.target`이 이를 포함합니다.
- `.path` 유닛: `PathExists=`, `PathExistsGlob=`, `DirectoryNotEmpty=`(유닛이 비활성인 동안 다시 확인), `PathChanged=`, `PathModified=`(inotify), `MakeDirectory=`, `TriggerLimitBurst=`.

**타이머**
- `systemd.time(7)` 문법 전체를 지원하는 `OnCalendar=`: 요일, 목록, 범위, `/step` 반복, `~`(월말 기준 날짜), 축약형, `UTC` 또는 IANA 시간대.
- `OnBootSec=`, `OnStartupSec=`, `OnActiveSec=`, `OnUnitActiveSec=`, `OnUnitInactiveSec=`.
- `Persistent=`(놓친 실행을 따라잡음), `RandomizedDelaySec=`, `FixedRandomDelay=`, `Unit=`.
- `systemctl list-timers`와 `systemctl status x.timer`가 다음/마지막 실행 시각을 보여줍니다.

**실패 처리 방식.** s6-rc에는 약한(soft) 의존성이 없습니다. 번들에 의존한다는 것은
그 안의 모든 서비스에 의존한다는 뜻이므로, 원하기만 한(wanted) 유닛 하나가 실패해도
그 뒤에 오는 모든 것이 멈춥니다. 그래서 sdcompat은 s6-rc를 순서 결정에만 씁니다.
- 모든 시작은 s6-rc에 성공으로 보고하고, 실제 결과(`failed`, `timeout` 등)는 따로 기록합니다.
- `Requires=`, `BindsTo=`, `Requisite=` 대상이 실패한 유닛은 시작하지 않으며, systemd처럼 작업 결과는 `dependency`가 됩니다.
- 유닛을 시작하면 실패했던 필수 유닛을 다시 시도하고, 이전에 조건 때문에 건너뛴 유닛은 조건을 다시 확인합니다.

**실행** (러너)
- `Type=simple|exec|notify|notify-reload|dbus|forking|oneshot|idle`. 유형마다 준비 완료(readiness) 신호를 s6의 notification fd에 연결합니다.
- `ExecStartPre/Post`, `ExecCondition`, `ExecStop`, `ExecStopPost`, `ExecReload`와 `-@+!:` 접두사, `$VAR`/`${VAR}` 확장.
- `User=`, `Group=`, `SupplementaryGroups=`. `DynamicUser=`는 영구 시스템 사용자로 근사합니다.
- `PAMName=`: systemd처럼 PAM 세션을 열고 자식 프로세스에서 서비스를 실행한 뒤, 종료되면 세션을 닫습니다. libpam은 실행 시점에 불러옵니다.
- `Delegate=`와 `DelegateSubgroup=`: 유닛의 cgroup을 서비스 사용자에게 넘깁니다(`systemd --user`에 필요).
- `Environment=`, `EnvironmentFile=`, `PassEnvironment=`, `UnsetEnvironment=`, `WorkingDirectory=`, `RootDirectory=`, `UMask=`, `Nice=`, `OOMScoreAdjust=`, `Limit*=`.
- `Runtime/State/Cache/Logs/ConfigurationDirectory=`.
- `StandardInput/Output/Error=`(null, tty, file:, append:, truncate:), `TTYPath=`, `TTYVHangup=`. `getty@.service`에 필요한 만큼 지원합니다.
- `Accept=no` 소켓 활성화: `ListenStream/Datagram/SequentialPacket/FIFO/Special`와 `LISTEN_FDS`, `LISTEN_PID`, `LISTEN_FDNAMES`. 소켓이 있는 서비스가 정상 종료하면 systemd처럼 소켓에 다음 활동이 있을 때 다시 시작됩니다(systemd-rfkill은 할 일이 없으면 종료합니다). `.socket`을 시작하면(`systemctl start`, udev의 `SYSTEMD_WANTS`) 그 서비스가 소켓과 함께 시작됩니다.
- `Restart=`(모든 모드), `RestartSec=`, `SuccessExitStatus=`, `RestartPrevent/ForceExitStatus=`, `StartLimitBurst/IntervalSec=`.
- `OnFailure=`, `OnSuccess=`, `SuccessAction=`, `FailureAction=`(reboot/poweroff/halt).
- `FDSTORE=1` / `FileDescriptorStoreMax=`: 저장된 fd를 보관했다가 재시작 후 돌려줍니다(journald와 logind가 사용). 리스닝 소켓도 같은 방식으로 보관하므로 서비스를 재시작해도 열린 채로 유지됩니다. 저장소는 별도의 s6 서비스 `sdcompat-fdstore`(`sdcompat fdstore`)이므로 sdcompatd를 다시 시작해도 fd가 남습니다. 이 서비스가 없으면 데몬이 직접 보관합니다. `FileDescriptorStoreMax=`가 있는 서비스는 이 서비스 다음에 시작됩니다.
- `TimeoutStartSec=`는 러너가 직접 적용합니다. 시간을 넘기면 시작이 실패하고 서비스는 종료됩니다.
- 출력은 stdout 스트림 프로토콜을 통해 **systemd-journald**로 보냅니다(PID 1이 하는 방식 그대로). journald가 cgroup으로 유닛을 식별하므로 `journalctl -u UNIT`과 `systemctl status`의 로그 줄이 동작합니다. `JOURNAL=no`이면 서비스마다 s6-log 디렉터리를 씁니다.
- 조건과 단언: 경로, 커널 명령줄, 가상화, 보안 등에 대한 `Condition*=`/`Assert*=`.
- systemd와 같은 경로(`system.slice/<unit>`)의 cgroup v2, `KillMode=`, `MemoryMax/High`, `CPUQuota/Weight`, `TasksMax`, `IOWeight`.
- 샌드박스: `PrivateTmp`, `ProtectSystem`, `ProtectHome`, `Read{Write,Only}Paths`, `InaccessiblePaths`, `NoNewPrivileges`, `CapabilityBoundingSet`, `AmbientCapabilities`, `PrivateDevices`, `PrivateNetwork`, `PrivateIPC`, `PrivateMounts`, `ProtectKernelTunables`, `ProtectKernelModules`, `ProtectKernelLogs`, `ProtectControlGroups`, `ProtectClock`, `ProtectHostname`, `ProtectProc`, `ProcSubset`. 마운트, capability 제거, 시스템 콜 필터, 암묵적인 `NoNewPrivileges=`는 systemd와 같습니다. 차이점: 유닛의 각 프로세스(`ExecStartPre=`, 주 프로세스, `ExecStop=`)가 네트워크/IPC/UTS 네임스페이스를 따로 받습니다(systemd는 유닛마다 하나를 공유). `ProtectControlGroups=private|strict`는 cgroup 트리를 읽기 전용으로만 만듭니다(cgroup 네임스페이스 없음). 나머지 지시어는 보고서에 "ignored"로 표시됩니다.
- seccomp: `SystemCallFilter=`, `SystemCallErrorNumber=`, `SystemCallArchitectures=`. systemd의 시스템 콜 그룹(`@system-service` 등, `systemd-analyze syscall-filter`에서 만든 `src/syscall-groups.txt`)을 씁니다. 또한 `RestrictAddressFamilies=`, `RestrictNamespaces=`, `MemoryDenyWriteExecute=`(커널에 `PR_SET_MDWE`가 있으면 systemd처럼 그것을 사용), `LockPersonality=`, `RestrictRealtime=`, `RestrictSUIDSGID=`를 systemd와 같은 규칙과 errno로 적용합니다. systemd와 마찬가지로 `RestrictAddressFamilies=`는 i386 ABI에는 적용되지 않습니다(`socket()`이 `socketcall()`을 거침). libseccomp는 실행 중에 불러옵니다.
- cgroup BPF: `DevicePolicy=`, `DeviceAllow=`(`/dev/` 노드, 그리고 /proc/devices의 `char-`/`block-` 분류, 글롭 포함. `PrivateDevices=`는 `DevicePolicy=closed`를 뜻함), `IPAddressDeny=`, `IPAddressAllow=`(주소, 프리픽스, `any`, `localhost`, `link-local`, `multicast`). systemd처럼 러너가 손으로 조립한 eBPF 프로그램(`sd_devices`, `sd_fw_ingress`, `sd_fw_egress`)을 유닛의 cgroup에서 아무것도 실행되기 전에 붙입니다. 커널이 거부한 프로그램은 시작을 실패시키지 않고 로그만 남깁니다(`... not enforced: ...`). 차이점: 주소 목록을 BPF 맵 대신 프로그램 안에 넣어 컴파일하고, 슬라이스에 설정한 목록은 적용하지 않습니다.
- SELinux는 systemd에서와 같습니다. s6 트리 전체가 `init_t`로 실행되므로(정책 모듈 `dist/selinux/sdcompat.cil`) 서비스는 각자의 도메인(`chronyd_t`, `xdm_t` 등)으로 들어가고, `user@.service`는 `systemd --user`를 사용자 도메인에서 시작할 수 있습니다. `RuntimeDirectory=` 등의 디렉터리와 소켓 파일은 file_contexts의 레이블을 받고, 소켓 유닛의 소켓은 그것을 쓰는 서비스의 레이블을 가집니다.

**D-Bus API** (`systemctl`과 라이브러리가 호출하는 것)
- 작업: `Start/Stop/Restart/TryRestart/Reload/ReloadOrRestart…Unit`, `JobNew`/`JobRemoved` 시그널, systemd의 작업 결과(`done`, `failed`, `dependency`, `timeout`). isolate도 지원합니다.
- 조회: `GetUnit`, `LoadUnit`, `GetUnitByPID`, `ListUnits*`, `ListJobs`, `GetUnitProcesses`. Unit, Service, Target, Scope, Job 속성을 제공하며 `GetAll("")`도 지원합니다.
- 유닛 파일: `Enable/Disable/Reenable/Mask/Unmask/Preset*UnitFiles`, `GetUnitFileState`, `ListUnitFiles*`, `Get/SetDefaultTarget`.
- 매니저: `Reload`/`Reexecute`(s6 데이터베이스 재생성 및 갱신), `KillUnit`, `ResetFailed*`, `Set/UnsetEnvironment`, `PowerOff/Reboot/Halt`.
- 패키지 설치와 업데이트: systemd RPM의 파일 트리거가 `systemctl daemon-reload`를 실행하고, 이것이 s6 데이터베이스를 재생성하므로 새로 설치된 유닛은 바로 s6 서비스가 됩니다(dnf, GNOME 소프트웨어, PackageKit 모두 같습니다. 데이터베이스는 부팅할 때마다 재생성되기도 합니다). 업데이트 때는 패키지 스크립트렛이 유닛에 `Markers=+needs-restart`/`+needs-reload`를 표시하고(`SetUnitProperties`), 이어서 `systemctl enqueue-marked`(`EnqueueMarkedJobs`)가 실행 중인 유닛을 systemd처럼 재시작하거나 다시 읽게 합니다. 사용자 유닛의 경우 파일 트리거가 `user@*.service`를 reload하고(사용자 매니저가 `ReloadSignal=`을 받아 다시 실행됩니다. 모든 `Type=notify-reload`와 마찬가지로 reload는 `RELOADING=1`/`READY=1`을 기다립니다), 각 사용자 매니저에서 `systemctl --user -M UID@ enqueue-marked`를 실행합니다. 이것은 `systemd-run --pipe`(표준 입출력을 파일 디스크립터로 전달)와 `systemd-stdio-bridge`를 거칩니다.
- 임시(transient) 유닛: `.scope`와 `.slice` 유닛에 대한 `StartTransientUnit`(`PIDs` 또는 `PIDFDs`; logind, podman, flatpak이 사용). `AttachProcessesToUnit`, `Scope.Abandon`, 참조 카운트 호출(수락만 함)도 지원합니다. scope와 slice는 `/run/sdcompat/units`에 기록되므로 다시 시작한 데몬이 넘겨받습니다(세션 scope가 계속 동작합니다).
- **`systemd-run`**: 보조 유닛을 포함한 임시 `.service`, `.timer`, `.path` 유닛(`--on-calendar`/`--on-active`는 타이머와 서비스를 함께 보냅니다). 속성은 `/run/systemd/transient/`의 유닛 파일이 됩니다. `systemd-run --wait`는 `PropertiesChanged`와 종료 상태를 받습니다. 끝난 임시 유닛은 언로드됩니다.
- 템플릿 인스턴스는 데이터베이스를 다시 컴파일하지 않고 필요할 때 만들어집니다(예: `user@1000.service`, `user-runtime-dir@1000.service`). 의존성이 먼저 시작되고, 멈춘 유닛을 필요로 하는 유닛은 함께 멈춥니다. 종료할 때 활성 상태인 oneshot 인스턴스를 멈춰 `ExecStop=`이 실행됩니다(systemd-backlight@가 밝기를 저장합니다). 이 인스턴스는 `/run/sdcompat/dyn-oneshot`에 기록되므로 데몬을 다시 시작한 뒤에도 마찬가지입니다. 다시 시작한 데몬은 실행 중인 인스턴스(`/run/sdcompat/dyn`의 서비스 디렉터리)도 넘겨받아 이전처럼 상태를 보고하고 멈춥니다.
- `poweroff.target`, `reboot.target`, `halt.target`을 시작하면(logind가 전원을 끄는 방식) 설정된 종료 명령을 실행합니다.
- D-Bus 활성화: dbus-daemon의 `ActivationRequest`에 응답하고, 시작이 실패하면 `ActivationFailure`로 답합니다.
- 권한: 상태를 바꾸는 호출에는 root, `ALLOW_UIDS`에 있는 uid, 또는 systemd의 action ID(`manage-units`, `manage-unit-files`, `reload-daemon`, `set-environment`; 정책 파일은 systemd에 포함)에 대한 polkit 허가가 필요합니다. polkit이 없으면 root가 아닌 호출자는 거부됩니다. `ALLOW_UIDS`는 root와 같은 권한입니다.
- D-Bus 활성화 요청은 버스 데몬 자신이 보낸 것만 받아들이며, 활성화 때문에 데이터베이스를 다시 만들지 않습니다.
- 같은 유닛의 작업은 차례로 실행되며, 대기 중인 작업은 취소할 수 있습니다.
- 장치가 원하는 유닛(udev의 `SYSTEMD_WANTS`, 예: 블루투스 어댑터의 `bluetooth.target`,
  `sound.target`, `bolt.service`)은 장치가 나타나면 시작됩니다. udevd의 이벤트와, 데몬이
  시작할 때는 udev 데이터베이스를 봅니다. 그중 제외된 유닛(`systemd-backlight@`,
  `systemd-rfkill`)은 건너뜁니다.

## 아직 구현되지 않은 것

- initramfs가 열지 않는 `/etc/crypttab`의 암호화 볼륨(systemd는 `systemd-cryptsetup@` 유닛으로 엽니다). LUKS 루트 디스크는 initramfs가 열기 때문에 동작하며, 나머지는 `setup-fedora.sh`가 경고합니다.
- `Accept=yes` 소켓과 첫 시작의 요청 시(on-demand) 활성화. 서비스는 소켓과 함께 미리 시작됩니다(그 뒤의 재시작만 소켓 활동을 기다립니다).
- autofs: `.automount` 유닛은 바로 마운트됩니다.
- `AccuracySec=`(타이머는 묶지 않고 제시간에 실행), `WakeSystem=`, `OnClockChange=`.
- 워치독, 자격 증명(`LoadCredential=`, `ImportCredential=`), `PrivateUsers=`, `PrivatePIDs=`, `RemoveIPC=`, `KeyringMode=`, `RestrictFileSystems=`(systemd에서는 BPF LSM 프로그램), IP 사용량 집계(`IPAccounting=`).
- 이미 실행 중인 작업의 취소.
- `ExecStop=`은 데몬을 통해 유닛을 멈출 때(`systemctl stop`)만 실행되고, s6-rc가 직접 서비스를 멈출 때(예: 종료 시)는 실행되지 않습니다.

## 오프라인 업데이트와 커널 업데이트

- **`dnf system-upgrade` / `dnf offline`:** `/system-update`를 만들고 재부팅합니다. 그러면 rc.init이
  `default` 대신 `system-update` 번들로 부팅합니다. 초기 부팅, D-Bus, sdcompat, logind,
  그리고 `system-update.target`이 포함되며, 이 타깃이 원하는 유닛들이 트랜잭션을
  실행합니다. systemd의 system-update 생성기가 하는 전환과 같습니다. 이 유닛들의
  `OnFailure=`/`SuccessAction=`도 따르며, 마지막 재부팅은 sdcompat의 `Reboot`을 거칩니다.
- **새 커널:** `/etc/kernel/install.d/90-sdcompat.install`이 Fedora의 grub 플러그인이 커널마다
  만드는 BLS 항목을 복사하고 `/etc/sdcompat/boot-options`의 s6 옵션을 추가합니다. s6 항목은
  커널 업데이트를 따라가고, 제거된 커널의 항목은 함께 지워집니다.

## 의존성

**필수** (`dist/setup-fedora.sh`가 설치합니다):

| 패키지 | 용도 |
|---|---|
| skalibs, execline, s6, s6-rc, s6-linux-init | PID 1, 서비스 감독, 서비스 데이터베이스. Fedora에 패키지가 없으므로 `--rpms=DIR` 또는 `--build-s6` |
| s6-frontend (선택) | `s6` 명령([`s6` 명령](#s6-명령-s6-frontend) 참고). 나머지와 함께 빌드됩니다 |
| systemd | 설치된 채로 둡니다: systemctl, udevd, journald, logind, `systemd --user`, tmpfiles, sysusers, libsystemd |
| dbus-daemon | 시스템 버스(`--systemd-activation`). 사용자 버스는 dbus-broker가 계속 맡습니다 |
| util-linux | 마운트 유닛과 초기 부팅의 `mount`, `swapon`, `mountpoint` |
| policycoreutils | SELinux 모듈용 `restorecon`, `semodule` |

**실행 중에 불러오는 라이브러리 (권장).** sdcompat이 `dlopen()`으로 불러오므로
빌드에는 필요 없습니다. 없으면 해당 기능만 빠집니다:

| 라이브러리 (Fedora 패키지) | 없을 때 |
|---|---|
| `libpam.so.0` (pam) | `PAMName=`이 있는 유닛이 실패(exit 224)하므로 `user@.service`와 GNOME이 시작되지 않습니다 |
| `libselinux.so.1` (libselinux) | `RuntimeDirectory=` 등, 소켓 파일, 소켓 유닛의 소켓에 레이블을 붙이지 않습니다. 서비스가 잘못된 레이블을 받을 수 있습니다 |
| `libseccomp.so.2` (libseccomp) | systemd가 seccomp 없이 빌드됐을 때처럼 `SystemCallFilter=`를 메시지와 함께 건너뜁니다. fwupd는 시작을 거부합니다 |

**권장 패키지:** earlyoom(systemd-oomd 대신), chrony, NetworkManager,
rsyslog(`JOURNAL=no`일 때만).

**빌드:** cargo와 rust. 크레이트는 zbus, tokio, futures-util, libc, serde입니다
(`Cargo.toml`).

## Fedora에서 시험하기

새로 클론하는 것부터 s6 위의 GNOME 세션까지, 확인할 것과 문제를 보고하는 방법을 단계별로
정리한 안내서: [TESTING.md](TESTING.md).

## root 없이 시험해 보기

s6, s6-rc, execline, dbus-daemon, busctl, python3가 필요합니다(Fedora에서는 먼저
skarnet RPM을 빌드하세요. 다음 절 참고). root로 실행되는 것은 없으며 호스트의 init
시스템은 건드리지 않습니다.

```sh
cargo build
tests/sandbox.sh ~/sdc-sandbox        # 별도의 s6-svscan, s6-rc, D-Bus 버스
tests/check.sh ~/sdc-sandbox          # 시나리오 검사 119개, 마지막은 비특권 호출자 단계 (샌드박스마다 한 번)
tests/systemctl.sh status hello.service   # 네임스페이스 안에서 진짜 systemctl 실행
tests/systemctl.sh list-units --all
tests/sandbox.sh ~/sdc-sandbox stop
```

`tests/systemctl.sh`는 호스트의 `systemctl`을 씁니다. systemctl이 없는 배포판에서는
압축을 푼 Fedora `systemd` + `systemd-shared` + `systemd-libs` RPM 경로를
`SYSTEMD_ROOT=`로 지정하세요. 이 구성은 Artix에서 Fedora 44의 systemctl 259.5로 시험했습니다.

## Fedora에 배포하기 (먼저 VM에서 해 보세요)

Fedora는 skalibs, execline, s6, s6-rc, s6-linux-init, s6-frontend를 패키지로 제공하지 않으므로 먼저
RPM을 빌드합니다(spec은 `dist/rpm/`에 있으며, 소스는 고정된 SHA-256 값으로 검증합니다).

```sh
dist/rpm/build-skarnet-rpms.sh                        # mock 그룹에 속해 있으면 mock 사용
dist/setup-fedora.sh --dry-run --rpms=$HOME/rpmbuild-skarnet/rpms
sudo dist/setup-fedora.sh --rpms=$HOME/rpmbuild-skarnet/rpms
```

`--build-s6`를 쓰면 대신 소스에서 `/usr`로 설치합니다(rpm이 관리하지 않음).

스크립트는 일반 systemd 부팅을 기본값으로 두고 GRUB 항목 "Fedora (s6 + sdcompat)"를
**추가**만 합니다. 덮어쓰는 파일은 `/var/lib/sdcompat/setup-backup/`에 백업합니다.
하는 일은 다음과 같습니다.

1. `dbus-daemon`, `earlyoom`, `chrony`, `NetworkManager`와 skarnet 도구를 직접 빌드한
   RPM(`--rpms=DIR`) 또는 소스(`--build-s6`)에서 설치합니다. 버전과 체크섬은
   `build-skarnet-rpms.sh`와 `--build-s6`가 함께 읽는 파일 하나, `dist/skarnet.versions`에
   고정되어 있습니다. 새 릴리스로 옮길 때는 그 파일만 고치면 됩니다. execline은 multicall
   바이너리 하나(`/usr/bin/execline`)로 빌드하므로 SELinux 모듈에 명령 목록이 없습니다.
2. sdcompat을 빌드하고 설치합니다. `dist/sdcompat.spec`으로 만든 sdcompat RPM이 설치되어 있으면 그것을 씁니다.
3. `/etc/sdcompat/`에 설정, s6 서비스(초기 부팅, dbus-daemon, 데몬, earlyoom), 부팅 옵션을
   쓰고, `s6` 명령용 `/etc/s6.conf`를 씁니다. systemd-journald는 s6 서비스로 실행됩니다. `--no-journal`을 주면 s6 전용 rsyslog
   설정을 쓰므로, journald가 `/dev/log`를 쓰는 systemd 부팅에는 영향이 없습니다. 이때 s6 서비스
   `journal-relay`가 journald의 클라이언트 소켓을 대신 열고 rsyslog로 넘깁니다(MESSAGE, 우선순위, 식별자만 남습니다).
4. chronyd, NetworkManager, gdm을 활성화하고 `graphical.target`을 기본값으로 설정합니다.
5. `sdcompat reload --no-update`를 실행하고 변환 요약을 출력합니다.
6. tty9에 복구용 getty가 있는 `/etc/s6-linux-init/current`를 생성합니다.
7. SELinux: `bin/init`에 `init_exec_t` 레이블을 붙이고 정책 모듈
   `dist/selinux/sdcompat.cil`을 불러옵니다. 이 모듈은 s6, s6-rc, s6-linux-init,
   execline 프로그램과 sdcompat 바이너리를 `init_exec_t`로 표시해서, systemd처럼
   s6 트리 전체가 `init_t`에 머물게 합니다. 또 systemd에는 없고 s6만 하는 몇 가지
   동작(catch-all 로거의 fifo, `/run/service`의 링크, /run/sdcompat에 있는
   `user@UID.service`의 run 스크립트)을 허용합니다. `--uninstall`이 모듈을 제거합니다.
8. kernel-install 플러그인을 설치하고, 이를 이용해 기본 부팅 항목을 복사하면서
   `init=/usr/sbin/init-selinux`를 추가합니다. plymouth의 종료 유닛은 systemd 전용이라
   스플래시를 제거합니다(`rhgb quiet`, `plymouth.enable=0`). `enforcing=0`도 추가하며,
   빼려면 `--enforcing`을 주세요.

그 밖의 옵션:

| 옵션 | 효과 |
|---|---|
| `--dry-run` | 모든 명령을 출력만 하고 아무것도 바꾸지 않습니다(root 불필요) |
| `--yes` | 시작 전에 묻지 않습니다 |
| `--rpms=DIR` | `dist/rpm/build-skarnet-rpms.sh`로 빌드한 RPM에서 skarnet 도구를 설치합니다 |
| `--build-s6` | 대신 skarnet 도구를 소스에서 `/usr`로 빌드합니다(rpm이 관리하지 않음) |
| `--rebuild` | `target/release/sdcompat`이 있어도 sdcompat을 다시 빌드합니다 |
| `--no-journal` / `--journal` | systemd-journald 없이 rsyslog와 서비스별 s6-log 디렉터리를 씁니다 / journald로 되돌립니다 |
| `--no-resolved` / `--resolved` | s6 부팅에서 systemd-resolved 없이 NetworkManager가 DNS를 직접 처리합니다 / resolved로 되돌립니다 |
| `--enforcing` | s6 부팅 항목에 `enforcing=0`을 추가하지 않습니다 |
| `--no-grub` | 부팅 항목을 추가하지 않습니다 |
| `--uninstall [--remove-s6]` | 부팅 항목과 설치된 파일(그리고 소스로 빌드한 skarnet 도구)을 제거합니다. 설정은 남깁니다 |

journal과 resolved 옵션은 이미 설치된 시스템도 전환합니다(`/etc/sdcompat/sdcompat.conf`의 `JOURNAL=`/`RESOLVED=`를 설정). 옵션 없이 다시 실행하면 거기 설정된 대로 유지합니다. `--no-resolved`이면 s6 oneshot `resolv`가 부팅 때 `/run/NetworkManager/conf.d/99-sdcompat-no-resolved.conf`(`dns=default`, `rc-manager=symlink`)를 쓰고, `/etc/resolv.conf`가 가리키는 resolved의 stub 파일을 `/run/NetworkManager/resolv.conf`로 연결합니다. 모두 `/run`에 있으므로 systemd 부팅은 계속 resolved를 씁니다. `/etc/NetworkManager`에 `dns=` 설정이 있으면 그것이 우선합니다(NetworkManager는 `/run`을 먼저 읽습니다). `/etc/nsswitch.conf`의 nss-resolve는 resolved가 없으면 `dns`로 넘어갑니다.

kernel-install 플러그인이 새 커널마다 s6 항목을 추가합니다.

어떤 systemd 구성 요소를 대체하는지, 그리고 logind와 `systemd --user`를 왜 남기는지
(GNOME에 필요합니다. 다음 절 참고):

| 역할 | s6 + sdcompat에서 |
|---|---|
| PID 1 / 서비스 | s6-linux-init, s6-rc, sdcompat |
| 시스템 버스 | dbus-daemon (사용자 버스용으로 dbus-broker는 유지) |
| 로그 | s6 서비스로 실행되는 systemd-journald (`--no-journal`: rsyslog + s6-log) |
| 네트워크 / DNS / 시간 | NetworkManager, systemd-resolved(`--no-resolved`: NetworkManager만), chrony |
| OOM 처리 | systemd-oomd 대신 earlyoom |
| 장치 | 일반 서비스로 실행되는 systemd-udevd |
| 로그인, 사용자 매니저 | 감독(supervised) 서비스로 실행되는 systemd-logind와 `systemd --user` |

2단계(stage 2)가 실패하면 tty9(Ctrl+Alt+F9)에서 로그인해 `/run/uncaught-logs/current`를
읽으세요. 마지막 변경 이전의 s6 데이터베이스로 부팅하려면 커널 명령줄에
`sdcompat.db=previous`를, 부팅 시 데이터베이스 재생성을 건너뛰려면 `sdcompat.reload=0`을
추가하세요.

skarnet 타르볼은 RPM 빌드 스크립트와 `--build-s6` 모두에서 고정된 SHA-256 값(2026-10-18
릴리스 기준)으로 검증합니다. 소스로 빌드한 경우 설치된 파일은 `--uninstall --remove-s6`을
위해 `/var/lib/sdcompat/skarnet-files.txt`에 기록되며, RPM으로 설치한 경우에는 평소처럼
`dnf remove`로 제거합니다.

### Fedora에서 테스트 실행하기

모든 테스트는 Fedora에서도 호스트의 `systemctl`과 `systemd-run`으로 그대로 동작합니다.
systemd로 부팅한 호스트에서도 네임스페이스 안에서 실행되므로 실제 systemd는 건드리지 않습니다.

```sh
dist/rpm/build-skarnet-rpms.sh && sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm
sudo dnf install dbus-daemon python3 cargo
cargo build && tests/sandbox.sh ~/sdc && tests/check.sh ~/sdc
tests/systemctl.sh status notifier.service
```

비특권 샌드박스로는 시험할 수 없는 부분(`User=`, cgroup, 샌드박스, 임시 scope)을
확인하려면 root 소유의 VM에서 설정 파일을 지정해 `sdcompat reload`를 실행하세요. 보고서와
`sdcompat plan`은 아무것도 시작하지 않고 Fedora 자체 유닛이 어떻게 변환되는지 보여줍니다.

```sh
sudo sdcompat --config /etc/sdcompat/sdcompat.conf plan | less
```

## GNOME과 사용자 매니저

GNOME 49는 세션 전체를 사용자 서비스 매니저를 통해 시작하고, gnome-shell은 그 안의
임시 scope에 애플리케이션을 배치합니다. 그래서 sdcompat은 `systemd --user`를 다시
구현하지 않습니다. **진짜 사용자 매니저를 일반 감독 서비스로 실행**하고, logind와
사용자 매니저가 쓰는 PID 1 API를 제공합니다. systemd는 여전히 PID 1이 아닙니다.
로그인할 때 일어나는 일:

1. GDM(또는 `login`)이 PAM 스택을 실행합니다. `pam_systemd`가 **systemd-logind**(Fedora 유닛에서 생성)에게 세션 생성을 요청합니다.
2. logind가 PID 1, 즉 sdcompat에게 `user-1000.slice`와 `session-N.scope` 생성(`PIDFDs`를 쓰는 `StartTransientUnit`)을 요청한 뒤, `user@1000.service` 시작을 요청합니다.
3. sdcompat이 `user@1000.service`를 그때 만듭니다. `BindsTo=user-runtime-dir@1000.service` 의존성이 먼저 실행됩니다. 러너가 `systemd-user` PAM 세션을 열고(`PAMName=`), `user.slice/user-1000.slice/user@1000.service`를 UID 1000에게 위임한 상태로 프로세스를 `init.scope`에 두며(`Delegate=`, `DelegateSubgroup=`), `/usr/lib/systemd/systemd --user`를 실행합니다. 준비 완료 신호는 `Type=notify-reload`로 받습니다.
4. 사용자 매니저가 세션 버스(dbus-broker 사용자 유닛)와 GNOME의 사용자 타깃을 시작합니다. 이 부분은 진짜 systemd이므로 세션 안에서 `systemctl --user`, `systemd-run --user`, 앱 scope가 모두 동작합니다.
5. 로그아웃하면 logind가 scope를, 이어서 `user@1000.service`를 멈춥니다. GNOME 메뉴의 전원 끄기와 재시작은 logind를 거쳐 `poweroff.target` 또는 `reboot.target`을 시작하며, sdcompat이 이에 맞는 종료 명령을 실행합니다.

위 단계에 더해 할 설정:

```sh
dnf install gdm gnome-shell gnome-session   # Workstation에는 이미 있습니다. 사용자 버스용 dbus-broker는 지우지 마세요
systemctl enable gdm.service                 # display-manager.service 별칭
systemctl set-default graphical.target
```

**상태:** 이 흐름의 D-Bus 쪽(`BindsTo=`가 있는 요청 시 인스턴스, 충돌, `Service=` 소켓,
종료 타깃)은 샌드박스 테스트로 확인했습니다. root가 필요한 부분(cgroup 위임, PAM 세션,
cgroup을 만드는 `StartTransientUnit`, 진짜 logind와 `systemd --user`)은 구현되어 있지만
**시험하지 않았습니다**. Fedora VM에서 시험해 보세요. 로그인이 실패하면
`journalctl -u systemd-logind`와 `journalctl -u user@1000.service`(또는 `JOURNAL=no`에서도
동작하는 `sdcompat log UNIT`)를 확인하세요.

## `s6` 명령 (s6-frontend)

설정 스크립트는 `s6` 명령을 제공하는 [s6-frontend](https://skarnet.org/software/s6-frontend/)
(버전은 `dist/skarnet.versions`)를 설치하고, sdcompat의 데이터베이스
(`bootdb = /var/lib/sdcompat/compiled`)를 쓰도록 `/etc/s6.conf`(`dist/s6.conf`)를 씁니다.
서비스 이름은 s6-rc의 이름입니다:
`sshd.service`, `gdm.service` 등과 sdcompat 자체의 s6 서비스(`dbus`, `sdcompatd`, `earlyoom`,
`udevd` 등).

| 명령 | 하는 일 |
|---|---|
| `s6 live status` | 모든 서비스와 종류, 상태 (root 불필요) |
| `s6 live status sshd.service` | 지정한 서비스의 상태 |
| `sudo s6 process status sshd.service` | 감독 중인 프로세스 (`s6-svstat`) |
| `sudo s6 process restart sshd.service` | 프로세스만 재시작하고, 그것에 의존하는 것은 그대로 둠 (`s6-svc -r`) |
| `sudo s6 process kill -s HUP sshd.service` | 프로세스에 시그널 보내기 |
| `sudo s6 live start\|stop X` | `s6-rc -u\|-d change X`. 유닛은 `systemctl`을 쓰세요(아래 참고) |
| `sudo s6 system reboot\|poweroff` | `systemctl reboot\|poweroff`와 같은 s6-linux-init 종료 |

`s6 live start/stop`과 `s6 process restart`는 sdcompat 아래에서 동작합니다. systemd 작업(job)이
없고 `ExecStop=`도 실행되지 않으므로(`s6-rc`를 직접 쓸 때와 같음), 유닛의 시작, 중지, 재시작에는
`systemctl`을 쓰세요.

`s6 repository …`, `s6 set …`, `s6 live install`, `s6 live start-everything`/`stop-everything`,
`s6 system boot`는 **쓰지 마세요**. 이 명령들은 s6-rc 저장소를 관리하고 live 데이터베이스를
바꾸거나 다시 시작하지만, sdcompat은 그 데이터베이스를 유닛 파일에서 직접 컴파일합니다
(`systemctl daemon-reload`, `sdcompat reload`).

## 설정

`/etc/sdcompat/sdcompat.conf`(`KEY=VALUE` 형식, 모든 키와 기본값은 `dist/sdcompat.conf` 참고).
가장 중요한 키:

- `EXCLUDE`, `EXCLUDE_ADD`, `INCLUDE`: 어떤 유닛을 변환할지 정합니다.
- `DEP_MAP`, `DEP_MAP_ADD`: systemd 유닛 → `EXTRA_SOURCE`의 s6 서비스 대응.
- `EXTRA_SOURCE`: 같은 데이터베이스로 함께 컴파일할 직접 작성한 s6-rc 소스.
- `JOURNAL`: `yes`(기본값)이면 systemd-journald를 실행하고 서비스 출력을 보냅니다. `no`이면 `LOG_DIR` 아래 서비스별 s6-log 디렉터리를 씁니다.
- `LOG_DIR`: `JOURNAL=no`일 때 s6-log 디렉터리가 들어갈 위치.
- `RESOLVED`: `yes`(기본값)이면 systemd-resolved를 실행합니다. `no`이면 그 유닛들을 `INCLUDE`에서 뺍니다(`setup-fedora.sh --no-resolved`처럼 `resolv` s6 서비스도 `sdcompat-sysinit`에 넣어야 DNS가 동작합니다).
- `ALLOW_UIDS`: 유닛을 관리할 수 있는 root 외 사용자.
- `UDEV_DIR`, `UDEV_MONITOR`: udev 데이터베이스 위치(`/run/udev`, 비우면 장치가 원하는 유닛을 시작하지 않음)와 udevd 이벤트를 따를지 여부.
- `INCLUDE`: 기본값으로 sdcompat에서 동작하는 systemd 서비스(logind, user-sessions, hostnamed, localed, timedated, resolved, vconsole-setup(콘솔 키맵과 글꼴), backlight와 rfkill(화면·키보드 밝기와 무선 상태를 부팅 사이에 유지), 절전 서비스)를 통과시킵니다.

## 소스 구조

```
src/unit.rs        유닛 파일 파서, 단어 분리, 시간 간격
src/loader.rs      검색 경로, 별칭, 템플릿, 드롭인, 지정자
src/gen.rs         계획 → s6-rc 소스, 컴파일, s6-rc-update, 보고서
src/runner.rs      실행 환경, 준비 완료 신호, 재시작 정책
src/install.rs     enable/disable/mask/preset, 유닛 파일 상태
src/socket.rs      소켓 유닛(sd_listen_fds)
src/sandbox.rs     네임스페이스, 마운트, capability
src/seccomp.rs     SystemCallFilter=와 나머지 seccomp 기반 설정(libseccomp)
src/bpf.rs         DevicePolicy=/DeviceAllow=, IPAddressDeny=/IPAddressAllow=(cgroup eBPF)
src/selinux.rs     파일과 소켓 레이블(libselinux)
src/cgroup.rs      systemd 호환 cgroup 구조와 자원 제어
src/conditions.rs  Condition*/Assert*
src/calendar.rs    OnCalendar= 파서와 다음 실행 시각 검색
src/timer.rs       타이머 러너(.timer마다 s6 longrun 하나)
src/pathunit.rs    path 유닛 러너(inotify)
src/mountunit.rs   .mount/.swap 유닛
src/pam.rs         dlopen(libpam)을 이용한 PAMName=
src/client.rs      데몬용 작은 클라이언트
src/fdstore.rs     파일 디스크립터 저장소(FDSTORE=1, 보관 중인 리스닝 소켓)
src/journal.rs     journal-relay: journald 클라이언트 소켓을 syslog로 전달(JOURNAL=no)
src/daemon/        D-Bus API: 디스패치, 속성, s6 백엔드, private 소켓,
                   요청 시 인스턴스(dynamic.rs), systemd-run(transient.rs),
                   장치가 원하는 유닛(devices.rs)
dist/              setup-fedora.sh, 설정, D-Bus 정책, s6 서비스, 초기 부팅 도우미,
                   s6-linux-init 골격, kernel-install 플러그인, sdcompat.spec
dist/rpm/          dist/skarnet.versions에 있는 skarnet 패키지의 spec과 빌드 스크립트
tests/             비특권 샌드박스, 시나리오 검사, systemctl 래퍼
```
