# Fedora에서 sdcompat 시험하기

[English](TESTING-us.md) | **한국어**

이 안내서는 `https://github.com/shandyCho/sdcompat`을 새로 클론하는 것부터, s6를
PID 1로 두고 Fedora의 GNOME 세션을 띄우는 것까지 다룹니다. 단계는 영향이 없는 것부터
시스템을 바꾸는 것 순서로 나뉘어 있습니다. 실패한 첫 단계에서 멈추고 알려 주세요
([문제 보고하기](#문제-보고하기) 참고).

| 단계 | root 필요 | 시스템 변경 | 소요 시간 |
|---|---|---|---|
| 1. 코드와 도구 받기 | 예 (dnf) | 빌드 도구 설치 | 10분 |
| 2. s6 RPM 빌드 | 예 (dnf) | 패키지 6개 설치 | 10분 |
| 3. 샌드박스 테스트 | 아니요 | 없음 | 5분 |
| 4. 변환 미리보기 | 예 (읽기 전용) | 없음 | 2분 |
| 5. 설치 | 예 | 부팅 항목, `/etc/sdcompat` 추가 | 10분 |
| 6. s6로 부팅 | 예 | 새 항목으로 부팅 | 15분 |

## 0. 시작하기 전에

- **가상 머신**에 **Fedora 44 또는 45 Workstation**(GNOME)을 설치해 쓰세요(s6 부팅은
  Fedora 45에서 시험했습니다). 메모리 4 GB,
  디스크 30 GB 이상이 좋습니다. 5단계 전에 **스냅샷을 찍어 두세요**. 부팅이
  잘못됐을 때 가장 빨리 되돌리는 방법입니다.
- `sudo`를 쓸 수 있는 일반 사용자로 로그인하세요.
- **root 비밀번호를 설정하세요.** Fedora Workstation은 기본적으로 root가 잠겨 있어서,
  설정하지 않으면 6단계의 복구 콘솔을 쓸 수 없습니다.
  ```sh
  sudo passwd root
  ```
- 부팅 항목을 고를 수 있도록 GRUB 메뉴가 항상 보이게 하세요.
  ```sh
  sudo grub2-editenv - unset menu_auto_hide
  ```

## 1. 코드와 빌드 도구 받기

저장소가 비공개이므로 먼저 GitHub에 로그인해야 합니다. GitHub CLI가 가장 간단합니다.

```sh
sudo dnf install -y git gh
gh auth login                      # GitHub.com, HTTPS를 고르고 브라우저로 로그인
gh repo clone shandyCho/sdcompat
cd sdcompat
git log -1 --oneline               # 보고할 때 쓸 커밋 해시를 적어 두세요
```

(GitHub에 SSH 키를 등록해 두었다면 `git clone git@github.com:shandyCho/sdcompat.git`도
됩니다.)

빌드와 테스트에 필요한 것을 모두 설치합니다.

```sh
sudo dnf install -y gcc make rpm-build cargo rust dbus-daemon python3 curl
```

전체 의존성 목록은 [README.md](README.md#의존성)에 있습니다. sdcompat은 libpam,
libselinux, libseccomp를 링크하지 않고 실행 중에 불러옵니다. Fedora Workstation에는
이미 있지만, 네 개가 모두 설치되어 있는지 확인하세요.

```sh
rpm -q pam libselinux libseccomp policycoreutils
```

## 2. s6 패키지 빌드와 설치

Fedora는 skalibs, execline, s6, s6-rc, s6-linux-init, s6-frontend를 패키지로 제공하지 않습니다.
빌드 스크립트가 소스를 내려받아 고정된 SHA-256 값으로 검증하고, 의존성 순서대로
RPM을 빌드합니다.

```sh
dist/rpm/build-skarnet-rpms.sh
```

- mock이 없으면 `rpmbuild`를 씁니다. 각 패키지를 `~/rpmbuild-skarnet/sysroot`에
  풀어 두고 다음 패키지는 그것을 이용해 빌드하므로, 아무것도 설치하지 않으며
  `sudo`도 필요 없습니다.
- mock을 쓰려면(`sudo dnf install mock && sudo usermod -aG mock $USER` 후 로그아웃했다가
  다시 로그인) 깨끗한 chroot 안에서 모두 빌드합니다.

RPM은 `~/rpmbuild-skarnet/rpms`에 생깁니다. 전부 설치하세요.

```sh
sudo dnf install -y ~/rpmbuild-skarnet/rpms/*.rpm
rpm -q skalibs execline s6 s6-rc s6-linux-init s6-frontend     # 여섯 개가 모두 나와야 합니다
```

**패키지 빌드가 실패하면** 로그가 `~/rpmbuild-skarnet/<이름>.build.log`에 있습니다.
마지막 50줄을 보내 주세요.

## 3. 샌드박스 테스트 (root 불필요, 시스템 변경 없음)

샌드박스는 별도의 s6-svscan, s6-rc, D-Bus 버스를 사용자 권한으로 실행합니다. 시스템이
systemd로 부팅되어 있어도 실행 중인 시스템은 건드리지 않습니다.

```sh
cargo build
tests/sandbox.sh ~/sdc
tests/check.sh ~/sdc 2>&1 | tee ~/sdc/check.out
```

**기대 결과:** 마지막 줄이 `passed: N  failed: 0`입니다(N은 120개 정도). 다음 줄은
정상이며 실패가 아닙니다.
- `SKIP transient scope (needs a writable cgroup …)`. cgroup을 만들려면 root가 필요합니다.
- `Call failed: Job type reload is not applicable …`. 오류가 제대로 반환되는지 확인하는 검사입니다.
- `tests/sandbox.sh`가 출력하는 `sdcompat[bad.service]: ExecStart=/bin/sh failed: exit status 1`.
  `bad.service`는 의존성 처리를 검사하려고 일부러 실패하게 만든 유닛입니다.

**알려진 간헐적 실패:** 실행의 절반 정도에서 같은 14개 검사가 실패합니다
(`hello.service: expected active/running, got inactive/dead`, `tick.timer: …`,
`job not queued`, `CancelJob failed` 등). 데이터베이스를 다시 불러올 때 생기는 알려진
버그입니다(`s6-rc-update: … Broken pipe`). 샌드박스를 새로 만들어 다시 실행하세요. 다른
검사가 실패하거나 이 14개가 매번 실패할 때만 알려 주세요.

`check.sh`는 샌드박스를 띄운 직후의 상태를 기대하므로 샌드박스마다 한 번만 실행됩니다. 다시
실행하려면 샌드박스를 새로 만드세요: `tests/sandbox.sh ~/sdc stop && tests/sandbox.sh ~/sdc`.

`JOURNAL=no` 설치(journald 대신 서비스별 s6-log)를 시험하려면 샌드박스를
`SANDBOX_JOURNAL=no tests/sandbox.sh ~/sdc`로 띄우세요. 그러면 `check.sh`는 서비스 출력을
journal 스트림 대신 `~/sdc/logs/` 아래에서 확인하고, `sdcompat journal-relay`로 native 메시지와
스트림 메시지를 보내 봅니다(소켓은 `$XDG_RUNTIME_DIR/sdc-test/jrelay`, 받은 내용은 대역 `/dev/log`가
같은 곳의 `devlog.out`에 씁니다).

이어서 Fedora의 진짜 `systemctl`로 샌드박스를 조작해 보세요.

```sh
tests/systemctl.sh status hello.service
tests/systemctl.sh list-units --all
tests/systemctl.sh list-timers
tests/sandbox.sh ~/sdc stop
```

**검사가 실패하면** 샌드박스를 멈추기 전에 로그를 모으세요.

```sh
tests/collect-diagnostics.sh ~/sdc
```

cgroup BPF 프로그램(`DevicePolicy=`, `IPAddressDeny=`)은 root 권한으로만 커널에서
확인할 수 있습니다. 이 테스트 하나는 sudo 암호를 묻고, 임시 cgroup
`/sys/fs/cgroup/sdcompat-bpf-test-PID`를 쓴 뒤 지우며, 어떤 유닛도 건드리지 않습니다:

```sh
tests/bpf-root.sh
```

**기대 결과:** `test result: ok. 1 passed`.

## 4. 변환 미리보기 (읽기 전용)

아무것도 시작하거나 쓰지 않고, 이 머신의 유닛을 sdcompat이 어떻게 변환할지 보여줍니다.

```sh
cargo build --release
sudo ./target/release/sdcompat plan > ~/sdcompat-plan.txt
head -3 ~/sdcompat-plan.txt
grep -A3 -E '^(gdm|NetworkManager|systemd-logind|chronyd|accounts-daemon)\.service ' ~/sdcompat-plan.txt
```

확인할 것:
- 첫 줄들: 변환된 유닛과 건너뛴 유닛의 수.
- 위 서비스들의 `warning:` 줄. 대부분 적용되지 않는 샌드박스 지시어이며 예상된 것입니다.
- 끝의 `skipped units:` 목록. `systemd-*` 유닛은 의도적으로 건너뜁니다.

## 5. 설치 (먼저 VM 스냅샷을 찍으세요)

미리 본 뒤 설치합니다.

```sh
dist/setup-fedora.sh --dry-run --rpms=$HOME/rpmbuild-skarnet/rpms
sudo dist/setup-fedora.sh --rpms=$HOME/rpmbuild-skarnet/rpms
```

스크립트는 계획을 보여주고, 무엇이든 바꾸기 전에 확인을 받습니다. 일반 systemd 부팅을
기본값으로 두고 **"Fedora (s6 + sdcompat)"** 부팅 항목을 추가합니다. 이 항목은 SELinux를
permissive 모드로 부팅합니다.

결과 확인:

```sh
ls /boot/loader/entries/ | grep sdcompat           # 새 부팅 항목
head -3 /var/lib/sdcompat/current/report.txt       # 변환 요약
```

## 6. s6로 부팅하기

```sh
sudo reboot
```

GRUB 메뉴에서 **Fedora (s6 + sdcompat) …**를 고르세요. 스플래시 화면 대신 텍스트 부팅
메시지가 보이고, 이어서 GDM 로그인 화면이 나옵니다.

**LUKS로 암호화했다면** s6 항목은 plymouth를 끄기 때문에, 비밀번호 입력란이 부팅 메시지
사이에 텍스트(`Please enter passphrase for disk …`)로 나타납니다. 평소처럼 그곳에
비밀번호를 입력하세요. 루트 디스크는 일반 부팅과 똑같이 initramfs가 엽니다. initramfs가
열지 않는 추가 암호화 볼륨은 s6에서 열리지 않으며, 설정 스크립트가 5단계에서 이를 경고합니다.

**GDM까지 가지 못하면** [s6 부팅이 실패할 때](#s6-부팅이-실패할-때)를 보세요.

GNOME에 로그인한 뒤 터미널을 열고 다음을 차례로 확인하세요.

| 확인 항목 | 명령 | 기대 결과 |
|---|---|---|
| s6가 PID 1 | `cat /proc/1/comm` | `s6-svscan` |
| SELinux 도메인 | `ps -eZ \| grep -E ' (s6-svscan\|chronyd\|gdm\|systemd-logind)$'` | `init_t`, `chronyd_t`, `xdm_t`, `systemd_logind_t` (`unconfined_service_t`가 아님) |
| SELinux 거부 | `sudo ausearch -m AVC -ts boot \| audit2allow` | 없는 것이 좋습니다. 있으면 출력을 보내 주세요 |
| fwupd | `fwupdmgr get-devices` | 장치 목록(fwupd가 시스템 콜 필터를 켠 채 시작됩니다) |
| 바쁜 러너 없음 | `top -b -n1 -o %CPU \| head -15` | CPU를 100 % 가까이 쓰는 `sdcompat` 프로세스가 없습니다 |
| earlyoom | `sudo s6-svstat /run/service/earlyoom` (몇 초 간격으로 두 번) | `up (pid …) N seconds`, N이 늘어납니다(재시작하지 않음) |
| 패키지 업데이트 시 재시작 | `systemctl show -p MainPID chronyd`, 이어서 `sudo dnf reinstall -y chrony`, 그다음 첫 명령을 다시 | MainPID가 바뀝니다(업데이트가 chronyd에 needs-restart를 표시하고 `systemctl enqueue-marked`가 재시작합니다) |
| 패키지 업데이트 시 사용자 매니저 | `sudo systemctl --user -M $USER@ is-system-running`, 이어서 `sudo /usr/lib/systemd/systemd-update-helper user-reload-restart; echo $?` | `running` 또는 `degraded`(`systemd-run --pipe`를 거쳐 도달), 그다음 `0` |
| 공유 마운트 전파 | `findmnt -no PROPAGATION /` | `shared`(systemd와 같습니다. `private`이면 나중에 한 마운트, 예를 들어 USB 메모리가 유닛·flatpak 앱·컨테이너에 보이지 않습니다) |
| 런타임 디렉터리 한 번만 마운트 | `findmnt -n /run/user/$UID \| wc -l` | `1`(`2`이면 user-runtime-dir@UID가 두 번 실행된 것입니다) |
| 멈춘 사용자 정리 | 로그인 후(그리고 `sudo` 후) 1분쯤 지나 `loginctl list-users` | `closing` 상태의 `root`·`gdm-greeter`가 남아 있지 않습니다(계속 `closing`이면 logind가 멈춤 작업이 아직 도는 줄 아는 것입니다) |
| /run 한 번만 마운트 | `findmnt -n /run \| wc -l` | `1`(systemd처럼 initramfs의 /run을 그대로 씁니다. `2`이면 s6-linux-init이 그 위에 tmpfs를 하나 더 올린 것입니다) |
| cgroup BPF 프로그램 | `sudo dnf install -y bpftool` 후 `sudo bpftool cgroup show /sys/fs/cgroup/system.slice/polkit.service; sudo bpftool cgroup show /sys/fs/cgroup/system.slice/systemd-logind.service` | polkit: `cgroup_device` 줄 하나(`sd_devices`). logind: `sd_devices`, `sd_fw_ingress`, `sd_fw_egress` 각각 하나 |
| 거부된 BPF 프로그램 | `journalctl -b -g 'not enforced'`(`JOURNAL=no`이면 `sudo grep 'not enforced' /var/log/messages`) | 아무것도 없음 |
| `DevicePolicy=strict`인 polkit | `pkaction \| head -3` | 액션 ID 세 개(polkitd가 응답함) |
| `s6` 명령 | `s6 live status \| head` | `up`인 서비스들, 예: `NetworkManager.service/longrun//up/explicit` |
| 버스와 userdb 레이블 | `ls -dZ /run/dbus/system_bus_socket /run/systemd/userdb` | `system_dbusd_var_run_t`와 `systemd_userdbd_runtime_t` |
| 전체 상태 | `systemctl is-system-running` | `running` (`degraded`도 괜찮습니다. 다음 줄 참고) |
| 실패한 유닛 | `systemctl --failed` | 없는 것이 좋습니다. 있으면 적어 두세요 |
| 저널 | `journalctl -b \| head` | 이번 부팅의 로그 |
| status의 로그 | `systemctl status NetworkManager` | 서비스의 최근 로그 줄이 보입니다 |
| 로그인 세션 | `loginctl` | 내 사용자의 세션 |
| 사용자 매니저 | `systemctl --user status` | 사용자 서비스 트리 |
| 네트워크 | `ping -c 3 fedoraproject.org` | 응답이 옵니다 |
| resolved 없는 DNS(`--no-resolved`일 때만) | `pgrep -x systemd-resolve; cat /etc/resolv.conf` | PID 없음, `# Generated by NetworkManager`와 네트워크의 네임서버 |
| 시간 동기화 | `chronyc tracking` | 기준 시간 소스가 있습니다 |
| 타이머 | `systemctl list-timers` | 다음 실행 시각이 있는 타이머들 |
| systemd-run | `sudo systemd-run --wait true` | `Finished with result: success` |
| 마운트 | `systemctl status tmp.mount` | `active (mounted)` |
| GNOME | 설정, 파일, 터미널을 열고 화면 잠금과 해제 | 모두 동작 |
| 전원 | 로그아웃 후 다시 로그인, 그다음 GNOME 메뉴로 전원 끄기 | 둘 다 동작 |

그다음 결과를 모으세요(모두 잘 동작해도 해 주세요).

```sh
sudo tests/collect-diagnostics.sh
```

**선택: systemd 부팅과 메모리 사용량 비교.** s6 부팅에서 한 번, 일반 systemd
부팅에서 한 번 실행하세요. 두 번 모두 GNOME에 로그인하고 같은 앱을 열어 둔
상태로 하세요.

```sh
sudo tests/memcompare.sh snap        # 부팅 후 5분이 될 때까지 기다립니다
```

실행할 때마다 `~/sdcompat-mem/mem-<s6|systemd>-<날짜>.tsv`가 생깁니다. 그다음
어느 부팅에서든:

```sh
tests/memcompare.sh compare ~/sdcompat-mem/mem-systemd-*.tsv ~/sdcompat-mem/mem-s6-*.tsv
```

시스템 전체 수치(`free`와 같은 방식), 그룹별 메모리(init과 감독, 로그, 사용자
매니저, 나머지), 메모리 차이가 가장 큰 명령들을 보여 줍니다. root로 실행하면
PSS(공유 페이지를 쓰는 프로세스들에 나눠 계산한 값)를 비교하고, root가 아니면
개인 익명 메모리로 비교합니다.

## s6 부팅이 실패할 때

1. **Ctrl+Alt+F9**를 누르세요. 나머지 부팅이 실패해도 그곳에서는 복구용 로그인
   프롬프트가 실행됩니다. root로 로그인한 뒤:
   ```sh
   tail -50 /run/uncaught-logs/current      # s6와 서비스가 출력한 내용
   s6-rc -a list                            # 실행 중인 것
   ```
2. 최신 s6 데이터베이스가 문제라면, 재부팅해서 GRUB의 s6 항목에서 **e**를 누르고
   `linux` 줄에 다음 중 하나를 추가하세요.
   - `sdcompat.db=previous`는 마지막 변경 이전의 데이터베이스로 부팅합니다.
   - `sdcompat.reload=0`은 부팅 시 데이터베이스 재생성을 건너뜁니다.
3. 빠져나오려면 재부팅해서 일반 Fedora 항목을 고르세요. journald가 실행됐었다면 실패한
   s6 부팅의 로그를 `journalctl -b -1`로 볼 수 있습니다. sdcompat 자신의 메시지(데몬,
   그리고 PAM 세션이나 exec 실패 같은 러너의 오류)도 그곳에 남습니다:
   `journalctl -b -1 -t sdcompatd -t sdcompat`.
4. 들어갈 수 있는 부팅에서 `sudo tests/collect-diagnostics.sh`를 실행하고 압축 파일을
   보내 주세요. 복구 콘솔만 쓸 수 있다면 `/run/uncaught-logs/current`를 보내 주세요.

VM 스냅샷으로 되돌리면 모든 변경이 사라집니다.

## 새 버전으로 업데이트하기

```sh
cd sdcompat
git pull
cargo build --release
sudo dist/setup-fedora.sh --rpms=$HOME/rpmbuild-skarnet/rpms --yes
```

설정 스크립트를 다시 실행해도 안전합니다. `/etc/sdcompat` 설정은 그대로 두고, 새
바이너리를 설치하고, 데이터베이스와 부팅 항목을 다시 만들며, 바꾸는 파일은 백업합니다.
`/etc/sdcompat/s6`의 s6 서비스는 직접 고치지 않았다면 새 버전으로 바뀝니다. 고친
서비스는 "keeping your modified …"를 출력하고 그대로 두니, `dist/s6`의 새 버전을 직접
합치세요. s6로 부팅한 상태에서 실행하면 sdcompatd를 재시작해서 재부팅 없이 새
바이너리가 동작합니다(sdcompat-fdstore와 journal-relay는 다음 부팅까지 이전 바이너리를
씁니다). SELinux 정책 모듈도 다시 불러옵니다. 불러와졌는지 확인하세요.

```sh
sudo semodule -l | grep -x sdcompat
```

업데이트에서 `dist/skarnet.versions`나 `dist/rpm/`이 바뀌었다면 먼저 s6 패키지를 다시 빌드해
설치하세요(2단계). 이 작업은 **일반 systemd 부팅**에서 하세요. 그러면 새 s6, s6-rc 프로그램이
이전 버전이 띄운 s6 트리를 건드리지 않고, 다음 s6 부팅에서 새 s6-rc로 데이터베이스를
컴파일합니다.

## 제거하기

일반 Fedora 항목으로 부팅한 뒤:

```sh
sudo dist/setup-fedora.sh --uninstall
sudo dnf remove skalibs execline s6 s6-rc s6-linux-init s6-frontend \
    skalibs-devel execline-devel s6-devel s6-rc-devel s6-linux-init-devel
```

`/etc/sdcompat`, `/var/lib/sdcompat`, 활성화한 유닛은 남습니다. systemd 부팅에서는 아무
영향이 없습니다.

## 문제 보고하기

다음을 함께 보내 주세요.
1. 커밋 해시(`git log -1 --oneline`)
2. 단계와 실패한 정확한 명령
3. 기대한 결과와 실제로 일어난 일
4. `tests/collect-diagnostics.sh`가 만든 진단 압축 파일(3단계:
   `tests/collect-diagnostics.sh ~/sdc`, 6단계: `sudo tests/collect-diagnostics.sh`).
   보내기 전에 내용을 확인하세요(`tar -tzf 파일`). 시험 머신의 로그, 유닛 이름, 부팅
   설정이 들어 있습니다.
