# sdcompat

[English](README-us.md) | **한국어**

s6를 위한 systemd 호환 계층입니다. Fedora는 **s6-linux-init + s6-rc**가 실행하고,
관리는 s6 자체의 도구(`s6`, s6-rc의 저장소와 세트)로 합니다. sdcompat은 Fedora의 유닛
파일을 s6-rc 서비스로 변환하고, 그것이 필요한 프로그램을 위해 `systemctl`, libsystemd,
D-Bus 클라이언트가 계속 동작하게 합니다.

```
 systemctl / dnf 스크립틀릿 / libsystemd 클라이언트 / dbus-daemon 활성화
              │ org.freedesktop.systemd1 (시스템 버스 + /run/systemd/private)
              ▼
   sdcompatd ── 작업(job), 유닛 상태, enable/disable/preset, scope
              │ s6-rc -u/-d, s6-svc, s6-svstat
              ▼
   s6-rc 데이터베이스  ◄── sdcompat-gen reload: 유닛 파일 → s6-rc 스토어 →
              │          저장소 세트 "current" (s6 set ...) → 커밋, 설치
              │
   s6-supervise ── run/finish 스크립트가 `sdcompat-runner run|finish|up|down`,
                   `sdcompat-timers timer-run|path-run` 호출
                   (User=, Environment=, 소켓, NOTIFY_SOCKET, Restart= 등)
```

라이브러리를 바꿔치기하는 방식이 아니라 프로토콜 수준에서 동작합니다. 진짜
`systemctl`과 `libsystemd`가 systemd와 대화하듯 그대로 sdcompat과 대화합니다.

## 구성 요소

sdcompat은 역할별로 나뉘어 있습니다. **본체**는 Cargo 작업공간 하나(`crates/`)로,
서로 함께 동작하며 서로에게 의존할 수 있습니다. **독립 도구**(`standalone/`)는
각각 별도의 프로젝트로, 혼자 빌드·설치·실행되고, 본체는 이것들 없이도 동작합니다.

**본체 도구** (`/usr/libexec/sdcompat`에 설치):

| 도구 | 명령 | 역할 |
|---|---|---|
| `sdcompat-gen` | `reload`, `plan` | 모든 유닛을 s6-rc 스토어로 변환하고, 그것과 직접 작성한 스토어(`EXTRA_SOURCE`)로 s6-rc 저장소를 동기화한 뒤, 세트 `current`를 커밋해 부팅 및 live 데이터베이스로 설치합니다(`s6-rc-set-install`). 변환 보고서를 남깁니다. s6로 부팅된 상태에서는 `systemctl daemon-reload`처럼 `reload`가 sdcompatd에 맡깁니다. [`s6` 명령](#s6-명령-s6-frontend) 참고. |
| | `enable/disable/mask/unmask/preset/is-enabled` | 오프라인 유닛 파일 관리입니다. |
| `sdcompatd` | `daemon` | 시스템 버스 **그리고** `/run/systemd/private`에서 `org.freedesktop.systemd1`을 제공합니다(systemd 259 이상의 `systemctl`은 root로 실행될 때 이 소켓만 사용합니다). |
| | `start/stop/restart UNIT`, `log UNIT [-f]` | 데몬용 최소 클라이언트(private 소켓 또는 버스)와, 서비스 로그 보기(`journalctl -u`, `JOURNAL=no`이면 s6-log 디렉터리)입니다. |
| `sdcompat-runner` | `run/finish/up/down`, `stop-exec`, `reload-exec` | 생성된 s6 스크립트와 데몬이 호출합니다. systemd가 `fork()`와 `exec()` 사이에서 하는 일을 합니다. |
| `sdcompat-timers` | `timer-run`, `path-run` | `.timer`(다음 실행 시각 계산)나 `.path` 유닛(inotify 감시) 하나를 s6 longrun으로 실행하고, 데몬에 유닛 시작을 요청합니다. |
| `sdcompat` | 위의 모든 명령 | 사용자가 쓰는 명령(`/usr/bin/sdcompat`)입니다. 명령을 맡은 도구를 실행하므로 `sdcompat reload`도 그대로 동작합니다. |
| | `--version` | 자기 빌드와 설치된 각 도구의 빌드를 보여 줍니다. 본체 도구가 서로 다른 빌드이면 그렇게 표시하고 1로 끝납니다. 도구마다 `--version`도 있습니다. |

본체 도구는 각각 RPM 패키지가 되고, 패키지끼리 `Requires`로 의존합니다. 소스는 작업공간
하나에 그대로 있으며, 라이브러리 크레이트(`sdcompat-base`, `-install`, `-exec`, `-client`)는
빌드할 때만 쓰이고 패키지가 되지 않습니다. 패키지 구성과 빌드 방법:
[packages/README.md](packages/README.md).

**독립 도구** (선택, 각각 별도 프로젝트와 RPM):

| 프로젝트 | 역할 | 없으면 |
|---|---|---|
| `sdcompat-fdstore` | `FDSTORE=` fd와 리스닝 소켓을 별도 프로세스에서 보관합니다. 설정 파일과 유닛 파일을 읽지 않습니다. | sdcompatd가 직접 보관합니다. sdcompatd를 다시 시작하면 사라집니다. |
| `sdcompat-journal-relay` | `JOURNAL=no`일 때 journald의 클라이언트 소켓(`/run/systemd/journal/socket`과 `stdout`)을 열고 받은 내용을 rsyslog로 넘깁니다. `systemd --user`의 세션 버스인 dbus-broker는 이 소켓이 없으면 시작하지 않습니다. | `JOURNAL=no`에는 꼭 필요합니다. `JOURNAL=yes`(기본값)에서는 영향이 없습니다. |
| `sdcompat-linkwatch` | 새 감독자(supervisor)의 시작 이벤트가 듣기 전에 와 버려서 기다리기만 하는 daemon-reload(`s6-rc-update`)나 `s6-svlink`를 풀어 줍니다(s6의 경쟁 조건입니다. 이 대기는 끝나지 않고 s6-rc의 잠금을 쥐고 있습니다). 2초마다 `/proc`를 보고 10초 넘은 대기에 손을 씁니다. | 그런 reload가 멈추고, 그 뒤의 모든 s6-rc 명령도 멈춘 `s6-rc-update`를 죽일 때까지 멈춥니다. |
| `sdcompat-notify` | `systemd-notify` 대체입니다. `$NOTIFY_SOCKET`을 설정하는 어떤 서비스 관리자 아래에서도 동작합니다. | 계속 설치되어 있는 `systemd-notify`를 쓰면 됩니다. |

각각의 빌드·설치·실행 방법: [standalone/sdcompat-fdstore](standalone/sdcompat-fdstore/README.md),
[standalone/sdcompat-journal-relay](standalone/sdcompat-journal-relay/README.md),
[standalone/sdcompat-linkwatch](standalone/sdcompat-linkwatch/README.md),
[standalone/sdcompat-notify](standalone/sdcompat-notify/README.md).

## 구현된 기능

**변환** (`.service` → longrun/oneshot, `.target` → bundle, `.timer`/`.path` → longrun, `.mount`/`.swap` → oneshot)
- `Requires=`, `Requisite=`, `BindsTo=`, 그리고 `After=`/`Before=`와 함께 쓰인 `Wants=`는 s6-rc 의존성이 됩니다. 타깃은 `Wants=` 전부를 끌어옵니다.
- `.wants/`, `.requires/`, `.upholds/` 디렉터리, 별칭, 템플릿/인스턴스, 마스킹, 드롭인, 지정자(specifier).
- 암묵적 의존성: `DefaultDependencies=` → `sysinit.target`, `Type=dbus` → `dbus.socket`. `BusName=`이 있는(다른 `Type=`의) 서비스도 `dbus.socket` 다음에 시작합니다. systemd는 이런 서비스를 버스 활성화로 시작하지만, 여기서는 소켓 유닛이 부팅 때 시작할 수 있기 때문입니다.
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
  systemd처럼 시작할 때 사용자를 확인합니다. 정지할 때 이미 사라진 사용자(GDM의 greeter 사용자)는
  그 사용자로 실행할 ExecStop*= 명령만 실패시키고(217/USER) 정지 자체는 막지 않습니다.
- `PAMName=`: systemd처럼 PAM 세션을 열고 자식 프로세스에서 서비스를 실행한 뒤, 종료되면 세션을 닫습니다. libpam은 실행 시점에 불러옵니다.
- `Delegate=`와 `DelegateSubgroup=`: 유닛의 cgroup을 서비스 사용자에게 넘깁니다(`systemd --user`에 필요).
- `Environment=`, `EnvironmentFile=`, `PassEnvironment=`, `UnsetEnvironment=`, `WorkingDirectory=`, `RootDirectory=`, `UMask=`, `Nice=`, `OOMScoreAdjust=`, `Limit*=`.
- `Runtime/State/Cache/Logs/ConfigurationDirectory=`.
- `StandardInput/Output/Error=`(null, tty, file:, append:, truncate:), `TTYPath=`, `TTYVHangup=`. `getty@.service`에 필요한 만큼 지원합니다.
- `Accept=no` 소켓 활성화: `ListenStream/Datagram/SequentialPacket/FIFO/Special`와 `LISTEN_FDS`, `LISTEN_PID`, `LISTEN_FDNAMES`. 소켓이 있는 서비스가 정상 종료하면 systemd처럼 소켓에 다음 활동이 있을 때 다시 시작됩니다(systemd-rfkill은 할 일이 없으면 종료합니다). `.socket`을 시작하면(`systemctl start`, udev의 `SYSTEMD_WANTS`) 그 서비스가 소켓과 함께 시작됩니다.
- `Restart=`(모든 모드), `RestartSec=`, `SuccessExitStatus=`, `RestartPrevent/ForceExitStatus=`, `StartLimitBurst/IntervalSec=`.
- `OnFailure=`, `OnSuccess=`, `SuccessAction=`, `FailureAction=`(reboot/poweroff/halt).
- `FDSTORE=1` / `FileDescriptorStoreMax=`: 저장된 fd를 보관했다가 재시작 후 돌려줍니다(journald와 logind가 사용). 리스닝 소켓도 같은 방식으로 보관하므로 서비스를 재시작해도 열린 채로 유지됩니다. 저장소는 독립 도구 `sdcompat-fdstore`를 실행하는 별도의 s6 서비스이므로 sdcompatd를 다시 시작해도 fd가 남습니다. 이 도구를 설치하지 않았으면 데몬이 직접 보관합니다. `FileDescriptorStoreMax=`가 있는 서비스는 이 서비스 다음에 시작됩니다.
- `TimeoutStartSec=`는 러너가 직접 적용합니다. 시간을 넘기면 시작이 실패하고 서비스는 종료됩니다.
- 출력은 stdout 스트림 프로토콜을 통해 **systemd-journald**로 보냅니다(PID 1이 하는 방식 그대로). journald가 cgroup으로 유닛을 식별하므로 `journalctl -u UNIT`과 `systemctl status`의 로그 줄이 동작합니다. `JOURNAL=no`이면 서비스마다 s6-log 디렉터리를 씁니다.
- 조건과 단언: 경로, 커널 명령줄, 가상화, 보안 등에 대한 `Condition*=`/`Assert*=`.
- systemd와 같은 경로(`system.slice/<unit>`)의 cgroup v2, `KillMode=`, `MemoryMax/High`, `CPUQuota/Weight`, `TasksMax`, `IOWeight`. `MemoryPressureWatch=`/`MemoryPressureThresholdSec=`에 따라 systemd처럼 서비스에 `MEMORY_PRESSURE_WATCH`/`MEMORY_PRESSURE_WRITE`를 줍니다(cgroup의 `memory.pressure`, `off`이면 `/dev/null`). 이 변수가 없으면 sd-event를 쓰는 데몬이 /proc/pressure를 찾는데, 일부(systemd-hostnamed)는 SELinux가 이를 거부합니다.
- 샌드박스: `PrivateTmp`, `ProtectSystem`, `ProtectHome`, `Read{Write,Only}Paths`, `InaccessiblePaths`, `NoNewPrivileges`, `CapabilityBoundingSet`, `AmbientCapabilities`, `PrivateDevices`, `PrivateNetwork`, `PrivateIPC`, `PrivateMounts`, `ProtectKernelTunables`, `ProtectKernelModules`, `ProtectKernelLogs`, `ProtectControlGroups`, `ProtectClock`, `ProtectHostname`, `ProtectProc`, `ProcSubset`. 마운트, capability 제거, 시스템 콜 필터, 암묵적인 `NoNewPrivileges=`는 systemd와 같습니다. 차이점: 유닛의 각 프로세스(`ExecStartPre=`, 주 프로세스, `ExecStop=`)가 네트워크/IPC/UTS 네임스페이스를 따로 받습니다(systemd는 유닛마다 하나를 공유). `ProtectControlGroups=private|strict`는 cgroup 트리를 읽기 전용으로만 만듭니다(cgroup 네임스페이스 없음). 나머지 지시어는 보고서에 "ignored"로 표시됩니다.
- seccomp: `SystemCallFilter=`, `SystemCallErrorNumber=`, `SystemCallArchitectures=`. systemd의 시스템 콜 그룹(`@system-service` 등, `systemd-analyze syscall-filter`에서 만든 `crates/sdcompat-exec/src/syscall-groups.txt`)을 씁니다. 또한 `RestrictAddressFamilies=`, `RestrictNamespaces=`, `MemoryDenyWriteExecute=`(커널에 `PR_SET_MDWE`가 있으면 systemd처럼 그것을 사용), `LockPersonality=`, `RestrictRealtime=`, `RestrictSUIDSGID=`를 systemd와 같은 규칙과 errno로 적용합니다. systemd와 마찬가지로 `RestrictAddressFamilies=`는 i386 ABI에는 적용되지 않습니다(`socket()`이 `socketcall()`을 거침). libseccomp는 실행 중에 불러옵니다.
- cgroup BPF: `DevicePolicy=`, `DeviceAllow=`(`/dev/` 노드, 그리고 /proc/devices의 `char-`/`block-` 분류, 글롭 포함. `PrivateDevices=`는 `DevicePolicy=closed`를 뜻함), `IPAddressDeny=`, `IPAddressAllow=`(주소, 프리픽스, `any`, `localhost`, `link-local`, `multicast`). systemd처럼 러너가 손으로 조립한 eBPF 프로그램(`sd_devices`, `sd_fw_ingress`, `sd_fw_egress`)을 유닛의 cgroup에서 아무것도 실행되기 전에 붙입니다. 커널이 거부한 프로그램은 시작을 실패시키지 않고 로그만 남깁니다(`... not enforced: ...`). 차이점: 주소 목록을 BPF 맵 대신 프로그램 안에 넣어 컴파일하고, 슬라이스에 설정한 목록은 적용하지 않습니다.
- SELinux는 systemd에서와 같습니다. s6 트리 전체가 `init_t`로 실행되므로(정책 모듈 `dist/selinux/sdcompat.cil`) 서비스는 각자의 도메인(`chronyd_t`, `xdm_t` 등)으로 들어가고, `user@.service`는 `systemd --user`를 사용자 도메인에서 시작할 수 있습니다. `RuntimeDirectory=` 등의 디렉터리와 소켓 파일은 file_contexts의 레이블을 받고, 소켓 유닛의 소켓은 그것을 쓰는 서비스의 레이블을 가집니다.

**D-Bus API** (`systemctl`과 라이브러리가 호출하는 것)
- 작업: `Start/Stop/Restart/TryRestart/Reload/ReloadOrRestart…Unit`, `JobNew`/`JobRemoved` 시그널, systemd의 작업 결과(`done`, `failed`, `dependency`, `timeout`). isolate도 지원합니다.
- 조회: `GetUnit`, `LoadUnit`, `GetUnitByPID`, `ListUnits*`, `ListJobs`, `GetUnitProcesses`. Unit, Service, Target, Scope, Job 속성을 제공하며 `GetAll("")`도 지원합니다.
- 유닛 파일: `Enable/Disable/Reenable/Mask/Unmask/Preset*UnitFiles`, `GetUnitFileState`, `ListUnitFiles*`, `Get/SetDefaultTarget`.
- 매니저: `Reload`/`Reexecute`(s6 데이터베이스 재생성 및 갱신), `KillUnit`, `ResetFailed*`, `Set/UnsetEnvironment`, `PowerOff/Reboot/Halt`.
- 패키지 설치와 업데이트: systemd RPM의 파일 트리거가 `systemctl daemon-reload`를 실행하고, 이것이 s6 데이터베이스를 재생성하므로 새로 설치된 유닛은 바로 s6 서비스가 됩니다(dnf, GNOME 소프트웨어, PackageKit 모두 같습니다). 새 서비스는 systemd의 preset이 정한 대로 s6 세트에서 활성 또는 비활성으로 시작합니다. 업데이트 때는 패키지 스크립트렛이 유닛에 `Markers=+needs-restart`/`+needs-reload`를 표시하고(`SetUnitProperties`), 이어서 `systemctl enqueue-marked`(`EnqueueMarkedJobs`)가 실행 중인 유닛을 systemd처럼 재시작하거나 다시 읽게 합니다. 사용자 유닛의 경우 파일 트리거가 `user@*.service`를 reload하고(사용자 매니저가 `ReloadSignal=`을 받아 다시 실행됩니다. 모든 `Type=notify-reload`와 마찬가지로 reload는 `RELOADING=1`/`READY=1`을 기다립니다), 각 사용자 매니저에서 `systemctl --user -M UID@ enqueue-marked`를 실행합니다. 이것은 `systemd-run --pipe`(표준 입출력을 파일 디스크립터로 전달)와 `systemd-stdio-bridge`를 거칩니다.
- 임시(transient) 유닛: `.scope`와 `.slice` 유닛에 대한 `StartTransientUnit`(`PIDs` 또는 `PIDFDs`; logind, podman, flatpak이 사용). `AttachProcessesToUnit`, `Scope.Abandon`, 참조 카운트 호출(수락만 함)도 지원합니다. scope와 slice는 `/run/sdcompat/units`에 기록되므로 다시 시작한 데몬이 넘겨받습니다(세션 scope가 계속 동작합니다).
- **`systemd-run`**: 보조 유닛을 포함한 임시 `.service`, `.timer`, `.path` 유닛(`--on-calendar`/`--on-active`는 타이머와 서비스를 함께 보냅니다). 속성은 `/run/systemd/transient/`의 유닛 파일이 됩니다. `systemd-run --wait`는 `PropertiesChanged`와 종료 상태를 받습니다. 끝난 임시 유닛은 언로드됩니다.
- 템플릿 인스턴스는 데이터베이스를 다시 컴파일하지 않고 필요할 때 만들어집니다(예: `user@1000.service`, `user-runtime-dir@1000.service`). 의존성이 먼저 시작되고, 멈춘 유닛을 필요로 하는 유닛은 함께 멈춥니다. 종료할 때 활성 상태인 oneshot 인스턴스를 멈춰 `ExecStop=`이 실행됩니다(systemd-backlight@가 밝기를 저장합니다). 이 인스턴스는 `/run/sdcompat/dyn-oneshot`에 기록되므로 데몬을 다시 시작한 뒤에도 마찬가지입니다. 다시 시작한 데몬은 실행 중인 인스턴스(`/run/sdcompat/dyn`의 서비스 디렉터리)도 넘겨받아 이전처럼 상태를 보고하고 멈춥니다.
- 오래 실행되는 서비스의 `ExecStop=`은 s6-rc가 서비스를 직접 멈출 때도 실행됩니다: 종료 시, `s6 live stop`, `s6 live reset`, 패키지 업데이트로 서비스가 없어질 때. 이런 서비스마다 그 서비스에 의존하는 s6-rc oneshot `UNIT.stop`이 생기고(그래서 s6-rc가 이것을 먼저 내립니다), 그 `down`이 `TimeoutStopSec=` 안에서 `$MAINPID`와 함께 `ExecStop=`을 실행합니다. 데몬은 s6-rc가 서비스를 올려 두었으면 `UNIT.stop`도 올리고, `systemctl stop`은 `ExecStop=`을 이것에 맡깁니다(한 번만 실행). `RemainAfterExit=yes`인 oneshot은 원래부터 자기 `down`에서 실행했습니다. 차이점: 그 서비스에 의존하는 유닛은 `ExecStop=`보다 먼저가 아니라 나란히 멈춥니다(`UNIT.stop`에는 아무것도 의존하지 않습니다. 그렇게 하면 업그레이드 때 s6-rc-update가 새 서비스에 대한 의존성이 생긴 유닛을 모두 다시 시작합니다). 또 `s6 process restart`/`s6-svc`는 프로세스에만 작용하므로 그때는 `ExecStop=`이 실행되지 않습니다. `ExecStop=`이 있는 필요 시 인스턴스(longrun 포함)는 종료할 때 데몬이 멈춥니다.
- `poweroff.target`, `reboot.target`, `halt.target`을 시작하면(logind가 전원을 끄는 방식) 설정된 종료 명령을 실행합니다.
- D-Bus 활성화: dbus-daemon의 `ActivationRequest`에 응답하고, 시작이 실패하면 `ActivationFailure`로 답합니다.
- 권한: 상태를 바꾸는 호출에는 root, `ALLOW_UIDS`에 있는 uid, 또는 systemd의 action ID(`manage-units`, `manage-unit-files`, `reload-daemon`, `set-environment`; 정책 파일은 systemd에 포함)에 대한 polkit 허가가 필요합니다. polkit이 없으면 root가 아닌 호출자는 거부됩니다. `ALLOW_UIDS`는 root와 같은 권한입니다. 유닛 하나에 대한 호출은 systemd처럼 polkit에 `unit`과 `verb` 세부 정보를 넘기므로, 그것으로 판단하는 규칙이 동작합니다. systemd와 같이 `AttachProcessesToUnit`의 유닛 이름이 비어 있으면 호출자 자신의 유닛이며, 권한 없는 호출자도 자신의 `Delegate=yes` 유닛 안에서 자기 프로세스는 옮길 수 있습니다(`user@UID.service`의 `systemd --user`).
- SELinux 접근 검사를 systemd 262와 같이 polkit보다 먼저, root에게도 합니다. 호출자의 레이블(버스에서, 또는 private 소켓의 `SO_PEERSEC`)이 유닛 파일 레이블에 대해 클래스 `service`의 해당 권한(`start`, `stop`, `reload`, `status`)을, 매니저 호출과 유닛 파일이 없는 유닛은 sdcompatd 자신의 레이블에 대해 클래스 `system`의 권한(`reload`, `status`, `halt`, `reboot`, `enable`, `start`)을 가져야 합니다. `ListUnits`는 호출자가 볼 수 없는 유닛을 빼고 보여 줍니다. permissive 모드와 permissive 도메인에서는 거부를 기록만 합니다(sdcompatd의 stderr).
- D-Bus 활성화 요청은 버스 데몬 자신이 보낸 것만 받아들이며, 활성화 때문에 데이터베이스를 다시 만들지 않습니다.
- 같은 유닛의 작업은 차례로 실행되며, 대기 중인 작업은 취소할 수 있습니다.
- 장치가 원하는 유닛(udev의 `SYSTEMD_WANTS`, 예: 블루투스 어댑터의 `bluetooth.target`,
  `sound.target`, `bolt.service`)은 장치가 나타나면 시작됩니다. udevd의 이벤트와, 데몬이
  시작할 때는 udev 데이터베이스를 봅니다. 그중 제외된 유닛(`systemd-backlight@`,
  `systemd-rfkill`)은 건너뜁니다.

## 아직 구현되지 않은 것

- initramfs가 열지 않는 `/etc/crypttab`의 암호화 볼륨(systemd는 `systemd-cryptsetup@` 유닛으로 엽니다). LUKS 루트 디스크는 initramfs가 열기 때문에 동작하며, 나머지는 `sdcompat init`이 경고합니다.
- `Accept=yes` 소켓과 첫 시작의 요청 시(on-demand) 활성화. 서비스는 소켓과 함께 미리 시작됩니다(그 뒤의 재시작만 소켓 활동을 기다립니다).
- autofs: `.automount` 유닛은 바로 마운트됩니다.
- `AccuracySec=`(타이머는 묶지 않고 제시간에 실행), `WakeSystem=`, `OnClockChange=`.
- 워치독, 자격 증명(`LoadCredential=`, `ImportCredential=`), `PrivateUsers=`, `PrivatePIDs=`, `RemoveIPC=`, `KeyringMode=`, `RestrictFileSystems=`(systemd에서는 BPF LSM 프로그램), IP 사용량 집계(`IPAccounting=`).
- 이미 실행 중인 작업의 취소.

## 오프라인 업데이트와 커널 업데이트

- **`dnf system-upgrade` / `dnf offline`:** `/system-update`를 만들고 재부팅합니다. 그러면 rc.init이
  `default` 대신 `system-update` 번들로 부팅합니다. 초기 부팅, D-Bus, sdcompat, logind,
  그리고 `system-update.target`이 포함되며, 이 타깃이 원하는 유닛들이 트랜잭션을
  실행합니다. systemd의 system-update 생성기가 하는 전환과 같습니다. 이 유닛들의
  `OnFailure=`/`SuccessAction=`도 따르며, 마지막 재부팅은 sdcompat의 `Reboot`을 거칩니다.
- **새 커널:** kernel-install 플러그인 `/usr/lib/kernel/install.d/90-sdcompat.install`이 Fedora의 grub 플러그인이 커널마다
  만드는 BLS 항목을 복사하고 `/etc/sdcompat/boot-options`의 s6 옵션을 추가합니다. s6 항목은
  커널 업데이트를 따라가고, 제거된 커널의 항목은 함께 지워집니다.

## 의존성

**필수** (`sdcompat` 패키지들이 요구하므로 dnf가 설치합니다):

| 패키지 | 용도 |
|---|---|
| skalibs, execline, s6, s6-rc, s6-linux-init | PID 1, 서비스 감독, 서비스 데이터베이스. Fedora에 패키지가 없으므로 sdcompat COPR에서 받거나 `dist/rpm/build-skarnet-rpms.sh`로 빌드합니다 |
| s6-frontend (권장) | `s6` 명령([`s6` 명령](#s6-명령-s6-frontend) 참고). 나머지와 함께 빌드됩니다 |
| mdevd (권장) | s6 initramfs(`--boot-only-s6=yes`)에서 드라이버를 로드합니다. 나머지와 함께 빌드됩니다 |
| systemd | 설치된 채로 둡니다: systemctl, udevd, journald, logind, `systemd --user`, tmpfiles, sysusers, libsystemd |
| dbus-daemon | 시스템 버스(`--systemd-activation`). 사용자 버스는 dbus-broker가 계속 맡습니다 |
| util-linux | 마운트 유닛과 초기 부팅의 `mount`, `swapon`, `mountpoint`, 무선 상태의 `rfkill` |
| policycoreutils, policycoreutils-python-utils | SELinux 모듈용 `restorecon`, `semodule`. `sdcompat init`용 `semanage` |
| grubby | 부팅 항목(`sdcompat init`) |

**실행 중에 불러오는 라이브러리 (권장).** sdcompat이 `dlopen()`으로 불러오므로
빌드에는 필요 없습니다. 없으면 해당 기능만 빠집니다:

| 라이브러리 (Fedora 패키지) | 없을 때 |
|---|---|
| `libpam.so.0` (pam) | `PAMName=`이 있는 유닛이 실패(exit 224)하므로 `user@.service`와 GNOME이 시작되지 않습니다 |
| `libselinux.so.1` (libselinux) | `RuntimeDirectory=` 등, 소켓 파일, 소켓 유닛의 소켓에 레이블을 붙이지 않습니다. 서비스가 잘못된 레이블을 받을 수 있습니다. D-Bus 호출자에 대한 SELinux 접근 검사를 하지 않습니다 |
| `libseccomp.so.2` (libseccomp) | systemd가 seccomp 없이 빌드됐을 때처럼 `SystemCallFilter=`를 메시지와 함께 건너뜁니다. fwupd는 시작을 거부합니다 |

**권장 패키지:** earlyoom(systemd-oomd 대신), chrony, NetworkManager,
rsyslog(`JOURNAL=no`일 때만).

**빌드:** cargo와 rust. 본체는 zbus, tokio, futures-util, libc, serde를, 독립 도구는 libc만 씁니다
(`Cargo.toml`).

## Fedora에서 시험하기

새로 클론하는 것부터 s6 위의 GNOME 세션까지, 확인할 것과 문제를 보고하는 방법을 단계별로
정리한 안내서: [TESTING.md](TESTING.md).

## root 없이 시험해 보기

s6, s6-rc, execline, dbus-daemon, busctl, python3가 필요합니다(Fedora에서는 먼저
skarnet RPM을 빌드하세요. 다음 절 참고). root로 실행되는 것은 없으며 호스트의 init
시스템은 건드리지 않습니다.

```sh
cargo build                           # 본체. 독립 도구는 sandbox.sh가 빌드합니다
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

패키지를 설치한 뒤 `sdcompat init`으로 부팅을 설정합니다.

```sh
sudo dnf copr enable OWNER/sdcompat       # COPR 프로젝트. packages/README.md 참고
sudo dnf install sdcompat
sdcompat init --dry-run                   # 무엇을 할지 보기(root 불필요)
sudo sdcompat init
```

`dnf install sdcompat`은 도구 패키지 넷과 skarnet 패키지(skalibs, execline, s6, s6-rc,
s6-linux-init)를 설치하고, 권장 패키지로 s6-frontend, mdevd, earlyoom, rsyslog와 독립 도구도
설치합니다. COPR 없이 쓰려면 RPM을 직접 빌드해서(`dist/rpm/build-skarnet-rpms.sh`,
`packages/build-rpms.sh`, `standalone/*/build-rpm.sh`) `dnf install`하세요.
[packages/README.md](packages/README.md)를 참고하세요.

`sdcompat init`은 GRUB 항목 "Fedora (s6 + sdcompat)"를 **추가**하고, `--default-s6`을 주지
않으면 일반 systemd 부팅을 기본값으로 둡니다. 아무것도 설치하지 않습니다. 설정에 필요한 패키지가
없으면 멈추고 `dnf install` 명령을 알려 줍니다. 덮어쓰는 파일은 `/var/lib/sdcompat/setup-backup/`에
백업합니다. 계획을 보여 주고 물은 뒤 다음을 합니다.

1. 설치된 빌드를 보여 줍니다(`sdcompat --version`). 본체 패키지들은 한 빌드에서 나온 것이어야 합니다.
2. `/etc/sdcompat/`의 설정(`sdcompat.conf`의 `JOURNAL=`, `RESOLVED=`, `sdcompat-sysinit` 번들의
   s6 서비스)과 `s6` 명령용 `/etc/s6.conf`를 씁니다. s6 서비스(초기 부팅, dbus-daemon, 데몬,
   earlyoom)는 패키지가 설치합니다. systemd-journald는 s6 서비스로 실행됩니다. `--journal=no`를
   주면 s6 전용 rsyslog 설정을 쓰므로, journald가 `/dev/log`를 쓰는 systemd 부팅에는 영향이
   없습니다. 이때 s6 서비스 `journal-relay`가 journald의 클라이언트 소켓을 대신 열고 rsyslog로
   넘깁니다(MESSAGE, 우선순위, 식별자만 남습니다).
3. chronyd, NetworkManager, gdm을 활성화하고 `graphical.target`을 기본값으로 설정합니다.
4. `sdcompat reload --no-update`를 실행해 s6-rc 저장소와 세트 `current`(systemd의 활성화 상태에서 시작)를 만들고 부팅 데이터베이스를 설치한 뒤, 변환 요약을 출력합니다. 직접 작성할 s6-rc 서비스용 `/etc/s6/sources`도 만듭니다.
5. tty9에 복구용 getty가 있는 `/etc/s6-linux-init/current`를 생성합니다.
6. SELinux: `bin/init`에 `init_exec_t` 레이블을 붙이고 정책 모듈
   (`/usr/share/sdcompat/selinux/sdcompat.cil`, 원본은 `dist/selinux/`)을 불러옵니다. 이 모듈은 s6, s6-rc, s6-linux-init,
   execline 프로그램과 sdcompat 바이너리를 `init_exec_t`로 표시해서, systemd처럼
   s6 트리 전체가 `init_t`에 머물게 합니다. 또 systemd에는 없고 s6만 하는 몇 가지
   동작(catch-all 로거의 fifo, `/run/service`의 링크, /run/sdcompat에 있는
   `user@UID.service`의 run 스크립트)을 허용합니다. 모듈은 패키지를 설치할 때 불러오고 제거할 때 지웁니다.
7. s6 부팅 중이면 sdcompatd를 다시 시작해 설치된 빌드로 바꿉니다.
8. 설치된 커널마다 패키지의 kernel-install 플러그인을 실행해 부팅 항목을 복사하면서
   `init=/usr/sbin/init-selinux`를 추가합니다. plymouth의 종료 유닛은 systemd 전용이라
   스플래시를 제거합니다(`rhgb quiet`, `plymouth.enable=0`). 처음 실행할 때는 `enforcing=0`도
   추가하며, `--selinux=enforcing`을 주면 빠집니다.

옵션:

| 옵션 | 효과 |
|---|---|
| `--dry-run` | 모든 명령을 출력만 하고 아무것도 바꾸지 않습니다(root 불필요). 끝에 적용할 명령(같은 옵션에서 `--dry-run`만 뺀 `sudo sdcompat init ...`)을 보여줍니다 |
| `--yes` | 시작 전에 묻지 않습니다 |
| `--journal=yes\|no` | `no`: systemd-journald 없이 rsyslog와 서비스별 s6-log 디렉터리를 씁니다(`rsyslog`와 `sdcompat-journal-relay` 필요) / `yes`: journald로 되돌립니다 |
| `--resolved=yes\|no` | `no`: s6 부팅에서 systemd-resolved 없이 NetworkManager가 DNS를 직접 처리합니다 / `yes`: resolved로 되돌립니다 |
| `--selinux=enforcing\|permissive` | s6 항목의 SELinux 모드(`permissive`이면 `enforcing=0` 추가). 처음 실행할 때는 permissive |
| `--boot-only-s6=yes\|no` | `yes`: s6 항목을 Fedora의 initramfs 대신 sdcompat의 initramfs로 부팅해 systemd가 전혀 실행되지 않게 합니다(아래 참고, `mdevd` 필요) / `no`: Fedora의 것으로 되돌립니다 |
| `--default-s6` / `--default-systemd` | s6 항목을 기본 부팅 항목으로 합니다(나중에 설치되는 커널에도) / systemd 항목을 다시 기본값으로 합니다. 어느 쪽이든 systemd 항목은 메뉴에 남습니다 |
| `--grub=no` | 부팅 항목을 추가하거나 고치지 않습니다 |
| `--uninstall` | 부팅 항목, s6 initramfs 이미지, `/etc/s6-linux-init/current`, (init이 썼다면) `/etc/s6.conf`, 부팅 설정을 지웁니다. `/etc/sdcompat`은 남깁니다. 그다음 패키지를 `dnf remove`하세요 |

빠진 옵션은 지난 실행의 선택을 유지합니다. `/etc/sdcompat/sdcompat.conf`의
`JOURNAL=`/`RESOLVED=`, `/etc/sdcompat/boot-options`의 SELinux 모드,
`/etc/sdcompat/boot-only-s6`, `/etc/sdcompat/boot-default`를 읽습니다. 그래서 패키지를
업그레이드한 뒤 `sudo sdcompat init`만 실행하면 같은 선택으로 설정을 다시 합니다.

journal과 resolved 옵션은 이미 설치된 시스템도 전환합니다. `--resolved=no`이면 s6 oneshot `resolv`가 부팅 때 `/run/NetworkManager/conf.d/99-sdcompat-no-resolved.conf`(`dns=default`, `rc-manager=symlink`)를 쓰고, `/etc/resolv.conf`가 가리키는 resolved의 stub 파일을 `/run/NetworkManager/resolv.conf`로 연결합니다. 모두 `/run`에 있으므로 systemd 부팅은 계속 resolved를 씁니다. `/etc/NetworkManager`에 `dns=` 설정이 있으면 그것이 우선합니다(NetworkManager는 `/run`을 먼저 읽습니다). `/etc/nsswitch.conf`의 nss-resolve는 resolved가 없으면 `dns`로 넘어갑니다.

kernel-install 플러그인이 새 커널마다 s6 항목을 추가합니다. `--default-s6`이면 s6 항목을
기본값으로도 유지합니다. 커널을 업데이트하면 Fedora의 `20-grub.install`이 새 systemd 항목을
다음 기본값(`tmp_saved_entry`)으로 정하고 `95-set-boot-entry.install`이 그것을 저장하는데,
플러그인이 그 사이에 실행되어 새 s6 항목으로 바꿉니다. 기본 s6 항목의 커널이 제거되면 남은 s6
항목 중 가장 새 것을 고릅니다. `--uninstall`과 `--default-systemd`는 저장된 s6 기본값을 같은
커널의 systemd 항목으로 되돌립니다. 메뉴가 숨겨져 있을 때 systemd 항목을 고르려면 GRUB이
시작할 때 Shift를 누르고 있거나 Esc를 누르세요.

**`--boot-only-s6=yes`: systemd 없이 부팅.** Fedora의 initramfs는 dracut이 만들며, 루트로 전환할
때까지 systemd를 PID 1로 실행합니다. 이 옵션을 주면 s6 항목은 sdcompat의 initramfs로
부팅합니다(`/boot/initramfs-KVER-s6.img`. kernel-install 플러그인이 커널마다
`/usr/libexec/sdcompat/mkinitramfs`로 만들고, sdcompat 패키지를 업데이트하면 이미지가 있는
커널마다 다시 만듭니다. systemd 항목은 Fedora의 것을 그대로 씁니다). 그
`/init`(execline)은 **s6-svscan을 PID 1로** 실행하고, s6-svscan은 **mdevd**(skarnet의 uevent
데몬. `mdevd-coldplug`로 발견된 하드웨어의 드라이버를 장치의 modalias에 따라 로드합니다)와
`rootfs` 서비스를 감독합니다. 이 서비스는 커널 명령줄의 `root=`, `rootflags=`, `rootfstype=`,
`ro`/`rw`, `init=`, `rd.luks.uuid=`, `rd.luks.name=`을 읽고, 콘솔에서 LUKS 암호를 묻고(3회,
`/etc/crypttab`의 `discard`를 따릅니다), 루트를 `/sysroot`에 마운트한 뒤 s6-svscan을 멈춥니다.
그러면 s6-svscan의 `.s6-svscan/finish`가 `/dev`, `/proc`, `/sys`, `/run`을 옮기고 `init=`
(`/usr/sbin/init-selinux`)로 `switch_root`합니다. 실패하면 복구 셸을 열고, 셸을 나가면 다시
시도합니다. `rd.break`는 전환 직전에 셸을 주고, `sdcompat.initrd.timeout=초`는 디스크를 기다리는
시간(기본 30초)을 정합니다. dracut은 이미지를 만들 때 도구로만 쓰이며(`dracut-install`이
프로그램을 라이브러리와 함께, 모듈을 의존 모듈과 함께 복사합니다) 부팅 때는 아무것도 실행되지
않습니다. 제한: 루트는 입력한 암호로 열어야 하고(TPM2/FIDO2/PKCS#11 잠금 해제 없음), plymouth가
없으며(s6 항목은 원래 없음), 최대 절전 모드 복귀가 없고, 암호 입력에 US가 아닌 키맵을 쓰지
않습니다. 추가 모듈은 `/etc/sdcompat/initramfs-modules`에 적습니다(한 줄에 이름 또는 `=DIR`
하나). initramfs에서는 udev가 돌지 않으므로, systemd initramfs가 하듯 LUKS 매핑의 udev 활성화
기록을 `/run/udev/data`에 남겨 실제 시스템의 coldplug가 `/dev/disk/by-*` 링크를 만들게 합니다.

어떤 systemd 구성 요소를 대체하는지, 그리고 logind와 `systemd --user`를 왜 남기는지
(GNOME에 필요합니다. 다음 절 참고):

| 역할 | s6 + sdcompat에서 |
|---|---|
| Initramfs | Fedora의 것(dracut, systemd가 PID 1). `--boot-only-s6=yes`이면 sdcompat의 것: s6-svscan, mdevd, cryptsetup |
| PID 1 / 서비스 | s6-linux-init, s6-rc, sdcompat |
| 시스템 버스 | dbus-daemon (사용자 버스용으로 dbus-broker는 유지) |
| 로그 | s6 서비스로 실행되는 systemd-journald (`--journal=no`: rsyslog + s6-log) |
| 네트워크 / DNS / 시간 | NetworkManager, systemd-resolved(`--resolved=no`: NetworkManager만), chrony |
| OOM 처리 | systemd-oomd 대신 earlyoom |
| 무선 상태(rfkill) | systemd-rfkill 대신 s6 oneshot에서 util-linux `rfkill` |
| 장치 | 일반 서비스로 실행되는 systemd-udevd |
| 로그인, 사용자 매니저 | 감독(supervised) 서비스로 실행되는 systemd-logind와 `systemd --user` |

2단계(stage 2)가 실패하면 tty9(Ctrl+Alt+F9)에서 로그인해 `/run/uncaught-logs/current`를
읽으세요. 마지막 설치 이전의 s6 데이터베이스로 부팅하려면 커널 명령줄에
`sdcompat.db=previous`를 추가하세요. 부팅 때는 아무것도 다시 만들지 않습니다.

s6 종료 때 콘솔에 나온 내용은 다음 부팅에서 `/var/log/sdcompat/last-shutdown-console.log`
(s6의 catch-all 로그)와 `last-shutdown-kernel.log`(종료가 시작된 뒤의 커널 로그, auditd가 멈춘 뒤의
SELinux 거부 포함)로 볼 수 있습니다(root 전용). 종료가 시작되면 sdcompatd는 새 유닛 시작과 D-Bus
활성화를 systemd처럼 바로 거부합니다. 먼저 요청 시 시작된 인스턴스(user@UID, user-runtime-dir@UID,
systemd-backlight@ 등)를 logind와 D-Bus가 아직 실행 중일 때 `After=`에 맞게 멈추고, 그다음에
s6-rc가 서비스를 멈춥니다(이 첫 단계에는 따로 2분 제한이 있습니다). s6-rc가 시작하고 3분이 지나도
멈추지 않은 서비스는 강제로 종료되므로, 끝나지 않는 정지 때문에 기계가 꺼지지 않는 일은 없습니다.

skarnet 버전과 타르볼의 SHA-256 값(2026-10-18 릴리스 기준)은 RPM 빌드 스크립트가 읽는
파일 하나, `dist/skarnet.versions`에 고정되어 있습니다. 새 릴리스로 옮길 때는 그 파일만 고치면
됩니다. execline은 multicall 바이너리 하나(`/usr/bin/execline`)로 빌드하므로 SELinux 모듈에
명령 목록이 없습니다.

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

시스템은 s6 자체의 도구로 관리합니다. 설정 스크립트는 `s6` 명령을 제공하는
[s6-frontend](https://skarnet.org/software/s6-frontend/)(버전은 `dist/skarnet.versions`)를
설치하고, sdcompat의 저장소(repository), 스토어(store), 부팅 데이터베이스를 적은
`/etc/s6.conf`(`dist/s6.conf`)를 씁니다. 서비스 이름은 s6-rc의 이름입니다:
`sshd.service`, `gdm.service` 등과 sdcompat 자체의 s6 서비스(`dbus`, `sdcompatd`,
`earlyoom`, `udevd` 등).

구성 방식:

- sdcompat-gen은 유닛 파일을 s6-rc **스토어**(`/var/lib/sdcompat/store`)로 변환합니다.
  s6-rc **저장소**(`/var/lib/s6-rc/repository`)는 스토어 세 개를 이 순서로 연결합니다:
  변환된 유닛, sdcompat의 초기 부팅 서비스(`/etc/sdcompat/s6`), 직접 작성한 s6-rc
  서비스(`/etc/s6/sources`. 여기 있는 서비스는 앞 스토어의 같은 이름을 덮어씁니다).
- 저장소의 세트(set) **`current`**가 부팅할 것을 정합니다. 모든 서비스는 `always`(초기 부팅,
  `s6 live stop`으로 멈출 수 없음), `active`(활성: 부팅이 시작하는 `default` 번들에 들어감),
  `usable`(비활성), `masked`(데이터베이스에 아예 없음) 중 하나입니다.
- 처음 보는 서비스는 systemd의 상태에서 시작합니다. `default.target`이 끌어들이는 것
  (systemd가 활성화한 유닛)은 `active`, 나머지는 `usable`이 됩니다. **그다음부터는 세트가
  정합니다**: `systemctl enable/disable`은 여전히 systemd의 심볼릭 링크를 쓰지만
  부팅할 것을 바꾸지는 않습니다. `systemctl is-enabled`, `systemctl status`,
  `list-unit-files`는 세트를 보여 줍니다: `always`/`active`는 `enabled`, `usable`은
  `disabled`, `masked`는 `masked`입니다(`static`, `indirect`, `alias`는 systemd가 보는
  그대로입니다).
- `systemctl daemon-reload`와 `sdcompat reload`(패키지가 바뀌면 dnf의 파일 트리거도 실행)는
  스토어를 다시 만들고 저장소를 동기화한 뒤 `current`를 커밋해 부팅 및 live 데이터베이스로
  설치합니다. 커밋 전에 `s6-rc-set-fix -u`를 실행해 `active` 서비스가 의존하는 것을
  활성화하고(커밋은 일관성 없는 세트를 거부합니다), 옮긴 것을 출력합니다. 서비스가 더는
  `active`가 아닌데 남아 있는 그 서비스의 로거(`s6 set disable -p`는 의존 대상인 로거를
  그대로 둡니다)는 `usable`이 됩니다. 로거는 여전히 서비스와 함께 시작되고, 서비스를
  활성화하면 다시 `active`가 됩니다. 서비스의 `UNIT.stop` oneshot은 서비스와 같은 처방을
  받습니다(`always`이면 `active`). 서비스를 `-p` 없이 옮겼을 때도 마찬가지입니다.
- 부팅은 설치된 데이터베이스의 `default` 번들을 그대로 시작합니다. 부팅 때는 아무것도
  다시 만들지 않습니다. systemd 부팅에서 바꾼 유닛은 다음 reload 때 반영됩니다.

| 명령 | 하는 일 |
|---|---|
| `s6 set status [NAME...]` | `current`에서 각 서비스의 처방(prescription): `always`, `active`, `usable`, `masked` |
| `sudo s6 set enable\|disable -p NAME...` | 부팅할 것을 바꿉니다. `-p`는 그것에 의존하는 것도 함께 옮깁니다(없으면 세트가 일관성을 잃고, 다음 reload의 수정이 `disable`을 되돌립니다) |
| `sudo s6 set mask -p NAME...` | 서비스를 데이터베이스에서 뺍니다. 그것을 담은 타깃(번들)도 사라지므로 `disable`을 권합니다 |
| `sudo s6 set commit --no-snapshot` 후 `sudo s6 live install --no-snapshot` | `current`를 컴파일해 부팅 및 live 데이터베이스로 만듭니다 |
| `sudo s6 set apply --no-snapshot` | 커밋, 설치, live 상태를 세트에 맞추기를 한 번에 |
| `s6 live status [NAME...]` | 모든(또는 지정한) 서비스와 종류, 상태 (root 불필요) |
| `sudo s6 live start\|stop NAME` | `s6-rc -u\|-d change NAME` |
| `sudo s6 process status\|restart NAME` | 감독 중인 프로세스 (`s6-svstat`, `s6-svc -r`: 프로세스만, 그것에 의존하는 것은 그대로) |
| `sudo s6 process kill -s HUP NAME` | 프로세스에 시그널 보내기 |
| `sudo s6 system reboot\|poweroff` | `systemctl reboot\|poweroff`와 같은 s6-linux-init 종료 |

참고:

- **`--no-snapshot`**: s6-rc 0.8.0.2의 `s6-rc-set-install`은 세트의 스냅샷을 끝에
  초기화되지 않은 바이트가 하나 남은 경로로 확인합니다(`s6rc_repo_setuptodate()`가 경로를
  한 바이트 늦게 끝냅니다). 그래서 이 옵션이 없으면 방금 커밋한 세트를 무작위로
  "not up-to-date"라며 거부합니다. sdcompat은 항상 이 옵션을 넘깁니다. 업스트림 버그이며,
  sdcompat은 skarnet의 소스를 패치하지 않습니다.
- **초기 부팅 서비스**는 스토어가 그렇다고 할 때(`flag-essential`, 또는 `sdcompat init`이
  syslog, journal-relay, resolv에 대해 바꾸는 `sdcompat-sysinit` 소속) 정확히 `always`입니다.
  세트가 옮기면 reload가 되돌립니다.
- `s6 live start/stop`과 `s6 process restart`는 sdcompat 아래에서 동작합니다. systemd
  작업(job)이 없으므로 유닛의 시작과 중지는 `systemctl`이 낫습니다. `s6 live stop`은
  `ExecStop=`을 실행하고(`UNIT.stop`을 통해, 위 참고), `s6 process restart`는 실행하지 않습니다.
- 타깃(번들)을 시작하면 `usable` 구성원까지 모두 시작합니다(`s6-rc`와 같음). udev의 장치
  타깃(`bluetooth.target` 등)이 여기에 기댑니다.
- SELinux: `s6-rc-update`와 `s6-rc-format-upgrade`는 관리자 셸에서 실행되면 `init_t`로
  들어갑니다(`dist/selinux/sdcompat.cil`). 그래서 enforcing에서도 `s6 live install`과
  `s6 set apply`가 동작합니다. 이것이 없을 때는 스캔 디렉터리에 `init_t`가 읽지 못하는
  레이블이 붙어 설치가 멈췄습니다.
- 되돌리려면 `sdcompat.db=previous`로 부팅하세요(마지막 것 이전에 설치된 데이터베이스).

## 설정

`/etc/sdcompat/sdcompat.conf`(`KEY=VALUE` 형식, 모든 키와 기본값은 `dist/sdcompat.conf` 참고).
가장 중요한 키:

- `EXCLUDE`, `EXCLUDE_ADD`, `INCLUDE`: 어떤 유닛을 변환할지 정합니다.
- `DEP_MAP`, `DEP_MAP_ADD`: systemd 유닛 → `EXTRA_SOURCE`의 s6 서비스 대응.
- `EXTRA_SOURCE`: 생성된 스토어 다음에 저장소에 연결할 직접 작성한 s6-rc 스토어(기본값 `/etc/sdcompat/s6:/etc/s6/sources`).
- `REPO_DIR`, `STORE_DIR`, `SET`, `DEFAULT_BUNDLE`: 저장소(s6.conf의 `repodir`), 생성된 스토어, reload가 커밋하고 설치하는 세트, 그 커밋이 부팅용으로 만드는 번들(`default`).
- `JOURNAL`: `yes`(기본값)이면 systemd-journald를 실행하고 서비스 출력을 보냅니다. `no`이면 `LOG_DIR` 아래 서비스별 s6-log 디렉터리를 쓰고, s6 `syslog` 서비스가 rsyslogd이므로 배포판의 `rsyslog.service`는 제외합니다(`INCLUDE`로 되살릴 수 있습니다).
- `LOG_DIR`: `JOURNAL=no`일 때 s6-log 디렉터리가 들어갈 위치.
- `RESOLVED`: `yes`(기본값)이면 systemd-resolved를 실행합니다. `no`이면 그 유닛들을 `INCLUDE`에서 뺍니다(`sdcompat init --resolved=no`처럼 `resolv` s6 서비스도 `sdcompat-sysinit`에 넣어야 DNS가 동작합니다).
- `ALLOW_UIDS`: 유닛을 관리할 수 있는 root 외 사용자.
- `UDEV_DIR`, `UDEV_MONITOR`: udev 데이터베이스 위치(`/run/udev`, 비우면 장치가 원하는 유닛을 시작하지 않음)와 udevd 이벤트를 따를지 여부.
- `INCLUDE`: 기본값으로 sdcompat에서 동작하는 systemd 서비스(logind, user-sessions, hostnamed, localed, timedated, resolved, vconsole-setup(콘솔 키맵과 글꼴), backlight(화면·키보드 밝기를 부팅 사이에 유지), 절전 서비스)를 통과시킵니다. 무선 상태(rfkill)는 systemd-rfkill 대신 `rfkill` s6 oneshot이 유지합니다. 부팅 때 util-linux `rfkill`로 장치마다 soft block을 되살리고, 종료할 때 상태를 systemd-rfkill 자신의 파일(`/var/lib/systemd/rfkill/ID_PATH:TYPE`)에 저장하므로 systemd 부팅 항목과 상태를 공유합니다. systemd-rfkill과 달리 정상 종료할 때만 저장하며, 부팅 중 udev settle 뒤에 나타난 장치는 상태를 그대로 둡니다.

## 소스 구조

```
crates/                    본체 (Cargo 작업공간 하나)
  sdcompat-base/           본체의 모든 크레이트가 함께 쓰는 부분
    unit.rs                유닛 파일 파서, 단어 분리, 시간 간격
    loader.rs              검색 경로, 별칭, 템플릿, 드롭인, 지정자
    config.rs, envfile.rs  sdcompat.conf, EnvironmentFile= 형식
    state.rs               유닛별 상태 파일(RUN_DIR/units/<유닛>/)
    index.rs               생성된 데이터베이스의 색인(유닛 → s6-rc 서비스)
    fdstore.rs             fd 저장소 클라이언트와, 데몬이 대신 쓰는 저장소
    calendar.rs            OnCalendar= 파서와 다음 실행 시각 검색
    timerspec.rs, pathspec.rs, mountunit.rs   .timer/.path/.mount 유닛의 내용
  sdcompat-install/        enable/disable/mask/preset, 유닛 파일 상태
  sdcompat-exec/           서비스 프로세스를 준비하는 부분:
    sandbox.rs             네임스페이스, 마운트, capability
    seccomp.rs             SystemCallFilter=와 나머지 seccomp 기반 설정(libseccomp)
    bpf.rs                 DevicePolicy=/DeviceAllow=, IPAddressDeny=/IPAddressAllow=(cgroup eBPF)
    cgroup.rs              systemd 호환 cgroup 구조와 자원 제어
    selinux.rs, pam.rs     레이블과 접근 검사(libselinux), PAMName=(libpam), 둘 다 dlopen으로
    socket.rs              소켓 유닛(sd_listen_fds)
    execcmd.rs, conditions.rs   Exec*= 명령줄, Condition*/Assert*
  sdcompat-client/         데몬용 작은 D-Bus 클라이언트
  sdcompat-version/        --version이 보여 주는 빌드 버전(RPM 버전-릴리스 또는 git 커밋)
  sdcompat-gen/            계획 → s6-rc 소스, 컴파일, s6-rc-update, 보고서 (도구 sdcompat-gen)
  sdcompat-runner/         실행 환경, 준비 완료 신호, 재시작 정책 (도구 sdcompat-runner)
  sdcompat-timers/         타이머와 path 유닛 러너 (도구 sdcompat-timers)
  sdcompatd/               D-Bus API: 디스패치, 속성, s6 백엔드, private 소켓,
                           요청 시 인스턴스(dynamic.rs), systemd-run(transient.rs),
                           장치가 원하는 유닛(devices.rs), SELinux 접근 검사
                           (access.rs) (도구 sdcompatd)
  sdcompat/                알맞은 도구를 실행하는 sdcompat 명령과 `sdcompat init`
standalone/                별도 프로젝트. 각각 README와 spec이 있습니다
  sdcompat-fdstore/        파일 디스크립터 저장소 서버
  sdcompat-journal-relay/  journald 클라이언트 소켓을 syslog로 전달(JOURNAL=no)
  sdcompat-linkwatch/      시작 이벤트를 놓친 s6-rc-update 대기를 풀어 줌
  sdcompat-notify/         systemd-notify 대체
dist/                      설정, D-Bus 정책, s6 서비스, 초기 부팅 도우미,
                           s6-linux-init 골격, kernel-install 플러그인
packages/                  본체 도구의 RPM spec(도구마다 하나), 빌드 스크립트, copr-srpms.sh
dist/rpm/                  dist/skarnet.versions에 있는 skarnet 패키지의 spec과 빌드 스크립트
tests/                     비특권 샌드박스, 시나리오 검사, systemctl 래퍼
```
