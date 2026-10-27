# Fedora에서 uinitconverter 시험하기

[English](TESTING-us.md) | **한국어**

이 안내서는 `https://github.com/shandyCho/useless-init-converter`를 새로 클론하는 것부터, s6를
PID 1로 두고 Fedora의 GNOME 세션을 띄우는 것까지 다룹니다. 단계는 영향이 없는 것부터
시스템을 바꾸는 것 순서로 나뉘어 있습니다. 실패한 첫 단계에서 멈추고 알려 주세요
([문제 보고하기](#문제-보고하기) 참고).

| 단계 | root 필요 | 시스템 변경 | 소요 시간 |
|---|---|---|---|
| 1. 코드와 도구 받기 | 예 (dnf) | 빌드 도구 설치 | 10분 |
| 2. s6 RPM 빌드 | 예 (dnf) | 패키지 7개 설치 | 10분 |
| 3. 샌드박스 테스트 | 아니요 | 없음 | 5분 |
| 4. 변환 미리보기 | 예 (읽기 전용) | 없음 | 2분 |
| 5. 설치 | 예 | uinitconverter RPM 설치, 부팅 항목, `/etc/uinitconverter` 추가 | 15분 |
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

```sh
sudo dnf install -y git
git clone https://github.com/shandyCho/useless-init-converter.git
cd useless-init-converter
git log -1 --oneline               # 보고할 때 쓸 커밋 해시를 적어 두세요
```

빌드와 테스트에 필요한 것을 모두 설치합니다.

```sh
sudo dnf install -y gcc make rpm-build cpio cargo rust dbus-daemon python3
```

전체 의존성 목록은 [README.md](README.md#의존성)에 있습니다. uinitconverter는 libpam,
libselinux, libseccomp를 링크하지 않고 실행 중에 불러옵니다. Fedora Workstation에는
이미 있지만, 네 개가 모두 설치되어 있는지 확인하세요.

```sh
rpm -q pam libselinux libseccomp policycoreutils
```

## 2. s6 패키지 빌드와 설치

Fedora는 skalibs, execline, s6, s6-rc, s6-linux-init, s6-frontend, mdevd를 패키지로 제공하지 않습니다.
먼저 `dist/skarnet.versions`에 적힌 버전의 릴리스 타르볼을 skarnet.org에서 직접
`~/skarnet-sources`에 받습니다(파일 이름은 `이름-버전.tar.gz`. README.md의 "Fedora에
배포하기" 참고). 빌드 스크립트는 아무것도 내려받지 않고, 타르볼을 고정된 SHA-256 값으로
검증한 뒤 의존성 순서대로 RPM을 빌드합니다. 없는 타르볼이 있으면 그 파일 이름을 알려 주고
멈춥니다.

```sh
dist/rpm/build-skarnet-rpms.sh            # 다른 디렉터리의 타르볼: --sources DIR
```

- mock이 없으면 `rpmbuild`를 씁니다. 각 패키지를 `~/rpmbuild-skarnet/sysroot`에
  풀어 두고 다음 패키지는 그것을 이용해 빌드하므로, 아무것도 설치하지 않으며
  `sudo`도 필요 없습니다.
- mock을 쓰려면(`sudo dnf install mock && sudo usermod -aG mock $USER` 후 로그아웃했다가
  다시 로그인) 깨끗한 chroot 안에서 모두 빌드합니다.

RPM은 `~/rpmbuild-skarnet/rpms`에 생깁니다. 전부 설치하세요.

```sh
sudo dnf install -y ~/rpmbuild-skarnet/rpms/*.rpm
rpm -q skalibs execline s6 s6-rc s6-linux-init s6-frontend mdevd   # 일곱 개가 모두 나와야 합니다
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

**기대 결과:** 마지막 줄이 `passed: N  failed: 0`입니다(N은 260개 정도, `SANDBOX_JOURNAL=no`이면 조금 더 많습니다). 다음 줄은
정상이며 실패가 아닙니다.
- `SKIP transient scope (needs a writable cgroup …)`. cgroup을 만들려면 root가 필요합니다.
- `Call failed: Job type reload is not applicable …`. 오류가 제대로 반환되는지 확인하는 검사입니다.
- `tests/sandbox.sh`가 출력하는 `uinitconverter[bad.service]: ExecStart=/bin/sh failed: exit status 1`.
  `bad.service`는 의존성 처리를 검사하려고 일부러 실패하게 만든 유닛입니다.

**데이터베이스를 다시 불러올 때의 `Broken pipe`:** 부하가 걸린 기계에서는
`daemon.log`에 `s6-rc-update: fatal: unable to manage new service directories in …:
Broken pipe`와 그 뒤에 `uinitconverter: relinked …` 줄이 보일 수 있습니다. s6 2.15.1.1의
s6-ftrigrd 버그이고, uinitconverter가 고친 뒤 다시 시도하므로 검사는 통과해야 합니다. 그런데도
`hello.service: expected active/running, got inactive/dead` 같은 실패가 줄지어 나오면
`daemon.log`와 함께 알려 주세요.

`check.sh`는 샌드박스를 띄운 직후의 상태를 기대하므로 샌드박스마다 한 번만 실행됩니다. 다시
실행하려면 샌드박스를 새로 만드세요: `tests/sandbox.sh ~/sdc stop && tests/sandbox.sh ~/sdc`.

`JOURNAL=no` 설치(journald 대신 서비스별 s6-log)를 시험하려면 샌드박스를
`SANDBOX_JOURNAL=no tests/sandbox.sh ~/sdc`로 띄우세요. 그러면 `check.sh`는 서비스 출력을
journal 스트림 대신 `~/sdc/logs/` 아래에서 확인하고, `uinitconverter-journal-relay`로 native 메시지와
스트림 메시지를 보내 봅니다(소켓은 `$XDG_RUNTIME_DIR/sdc-test/jrelay`, 받은 내용은 대역 `/dev/log`가
같은 곳의 `devlog.out`에 씁니다).

uinitconverter의 SELinux 모듈이 설치된 머신에서는 `/usr/bin/s6-rc-update`를 직접 실행해도
`init_t`로 돕니다(`sudo s6 live install`이 동작하는 이유입니다). `init_t`는 샌드박스의 파일을 열
수 없으므로, `sandbox.sh`는 `s6-rc-update`와 `s6-rc-format-upgrade`를 `WORKDIR/bin`에 복사하고
(복사본은 그 디렉터리의 레이블을 가집니다), 두 스크립트 모두 그 디렉터리를 `PATH` 맨 앞에 둡니다.

본체가 독립 도구 `uinitconverter-fdstore` 없이 동작하는지 보려면 `SANDBOX_FDSTORE=no`를 앞에 붙여
`sandbox.sh`와 `check.sh`를 실행하세요. 그러면 데몬이 fd를 직접 보관하므로, 데몬을 다시 시작한
뒤 fd가 사라졌는지를 확인합니다. 독립 도구는 `tests/sandbox.sh`가 `standalone/`에서 빌드합니다.

시스템 버스를 dbus-daemon 대신 dbus-broker로 시험하려면 `SANDBOX_BUS=broker`를 앞에 붙여
`sandbox.sh`와 `check.sh`를 실행하세요(`dbus-broker`와 `unshare`가 필요합니다).
`dbus-broker-launch`는 소켓을 systemd 방식(`LISTEN_FDS`)으로 받고 항상
`/run/systemd/journal/socket`에 로그를 보내므로, 같은 uid의 사용자·마운트 네임스페이스에서
tmpfs `/run`과 대역 journal(`SANDBOX_JOURNAL=no`이면 journal-relay의 소켓, 아니면
`~/sdc/broker-journal.log`에 쓰는 소켓)을 붙여 실행합니다. 이 모드에서 `check.sh`는
`SystemdService=`가 없는 서비스가 `StartTransientUnit`의 transient unit
(`dbus-:1.N-org.example.Trans@K.service`)으로 시작되는지도 확인합니다.

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
`/sys/fs/cgroup/uinitconverter-bpf-test-PID`를 쓴 뒤 지우며, 어떤 유닛도 건드리지 않습니다:

```sh
tests/bpf-root.sh
```

**기대 결과:** `test result: ok. 1 passed`.

## 4. 변환 미리보기 (읽기 전용)

아무것도 시작하거나 쓰지 않고, 이 머신의 유닛을 uinitconverter가 어떻게 변환할지 보여줍니다.

```sh
cargo build --release
sudo ./target/release/uinitconverter plan > ~/uinitconverter-plan.txt
head -3 ~/uinitconverter-plan.txt
grep -A3 -E '^(gdm|NetworkManager|systemd-logind|chronyd|accounts-daemon)\.service ' ~/uinitconverter-plan.txt
```

확인할 것:
- 첫 줄들: 변환된 유닛과 건너뛴 유닛의 수.
- 위 서비스들의 `warning:` 줄. 대부분 적용되지 않는 샌드박스 지시어이며 예상된 것입니다.
- 끝의 `skipped units:` 목록. `systemd-*` 유닛은 의도적으로 건너뜁니다.

## 5. 설치 (먼저 VM 스냅샷을 찍으세요)

이 체크아웃으로 uinitconverter RPM을 빌드해 설치합니다(빌드에는 root가 필요 없습니다).

```sh
packages/build-rpms.sh
for b in standalone/*/build-rpm.sh; do "$b"; done
sudo dnf install ~/rpmbuild-uinitconverter/rpms/*.rpm ~/rpmbuild-uinitconverter-standalone/rpms/*.rpm
uinitconverter --version                    # 본체 도구가 모두 같은 빌드인지
```

(GitHub 릴리스의 패키지를 쓴다면, 한 릴리스의 `.rpm` 파일과 `SHA256SUMS`를 빈 디렉터리에
받아 `sha256sum -c SHA256SUMS && sudo dnf install ./*.rpm`을 실행하는 것이 위의 본체와 독립
도구 빌드를 대신합니다. skarnet 패키지는 릴리스에 없으므로 2단계는 그대로 합니다.)

패키지를 설치해도 부팅은 아직 바뀌지 않습니다. 미리 본 뒤 부팅을 설정합니다.

```sh
uinitconverter init --dry-run
sudo uinitconverter init
```

미리 보기는 실행할 명령을 모두 출력하고, 끝에 `Dry run: nothing was changed. To apply this,
run:`과 적용할 명령(`sudo uinitconverter init`, 옵션을 주었다면 같은 옵션)을 보여줍니다. 바뀌는 것은
없습니다. 빠진 패키지가 있으면 그 전에 `sudo dnf install ...`을 안내하고 끝납니다.

`uinitconverter init`은 계획을 보여주고, 무엇이든 바꾸기 전에 확인을 받습니다. **"Fedora (s6 + uinitconverter)"**
부팅 항목을 추가하고 일반 systemd 부팅을 기본값으로 둡니다(s6 부팅이 잘 되면 `--default-s6`으로
기본값으로 만들 수 있습니다. 이 문서 끝을 보세요). 첫 실행에서는 이 항목이 SELinux를
permissive 모드로 부팅하고, 다음 실행부터는 `--selinux=`를 주지 않는 한 그 모드를 유지합니다.

결과 확인:

```sh
sudo ls /boot/loader/entries/ | grep uinitconverter         # 새 부팅 항목
head -3 /var/lib/uinitconverter/current/report.txt       # 변환 요약
```

## 6. s6로 부팅하기

```sh
sudo reboot
```

GRUB 메뉴에서 **Fedora (s6 + uinitconverter) …**를 고르세요. 스플래시 화면 대신 텍스트 부팅
메시지가 보이고, 이어서 GDM 로그인 화면이 나옵니다.

**LUKS로 암호화했다면** s6 항목은 plymouth를 끄기 때문에, 비밀번호 입력란이 부팅 메시지
사이에 텍스트(`Please enter passphrase for disk …`)로 나타납니다. 평소처럼 그곳에
비밀번호를 입력하세요. 루트 디스크는 일반 부팅과 똑같이 initramfs가 엽니다. initramfs가
열지 않는 추가 암호화 볼륨은 s6에서 열리지 않으며, `uinitconverter init`이 5단계에서 이를 경고합니다.

**GDM까지 가지 못하면** [s6 부팅이 실패할 때](#s6-부팅이-실패할-때)를 보세요.

GNOME에 로그인한 뒤 터미널을 열고 다음을 차례로 확인하세요.

| 확인 항목 | 명령 | 기대 결과 |
|---|---|---|
| s6가 PID 1 | `cat /proc/1/comm` | `s6-svscan` |
| SELinux 도메인 | `ps -eZ \| grep -E ' (s6-svscan\|chronyd\|gdm\|systemd-logind)$'` | `init_t`, `chronyd_t`, `xdm_t`, `systemd_logind_t` (`unconfined_service_t`가 아님) |
| SELinux 거부 | `sudo ausearch -m AVC -ts boot \| audit2allow` | 없는 것이 좋습니다. 있으면 출력을 보내 주세요 |
| D-Bus 호출자의 SELinux 검사 | `sudo runcon -r system_r -t insights_core_t -- env systemctl show -p Version`, 그다음 `sudo runcon -r system_r -t insights_core_t -- env systemctl daemon-reload`, 그다음 `sudo grep -E 'SELinux access check\|avc: +denied' /run/uncaught-logs/current` | 두 명령 모두 아무것도 출력하지 않습니다. insights_core_t는 터미널에 쓸 수 없으므로(`user_devpts_t`에 대한 규칙 없음) 결과는 grep으로 봅니다. AVC의 `avc:  denied  { reload } … tclass=system permissive=0`과, 그다음 별도 줄에 `uinitconverterd: SELinux access check … perm=reload state=enforcing function=Reload`가 나옵니다(두 번째 명령은 root로 실행해도 systemd에서처럼 거부됨). 첫 명령에 대한 줄은 없고(그 도메인에는 `status`가 허용됨), 평소 사용에서 생긴 다른 줄도 없습니다(있으면 보내 주세요) |
| 디스패처 스크립트의 출력(`--journal=no`일 때만) | 재부팅 후 `sudo ausearch -m AVC -ts boot \| grep NetworkManager_dispatcher` | 아무것도 없음(전에는 s6-log 파이프에 쓰기가 거부됨) |
| fwupd | `fwupdmgr get-devices` | 장치 목록(fwupd가 시스템 콜 필터를 켠 채 시작됩니다) |
| 바쁜 러너 없음 | `top -b -n1 -o %CPU \| head -15` | CPU를 100 % 가까이 쓰는 `uinitconverter` 프로세스가 없습니다 |
| earlyoom | `sudo s6-svstat /run/service/earlyoom` (몇 초 간격으로 두 번) | `up (pid …) N seconds`, N이 늘어납니다(재시작하지 않음) |
| 요청 시 서비스 | `systemctl status virtlogd.service iscsid.service`와 `ls -l /run/libvirt/virtlogd-sock`. 그다음 `sudo python3 -c 'import socket; socket.socket(socket.AF_UNIX).connect("/run/libvirt/virtlogd-sock")'`을 실행하고 상태를 다시 봅니다 | 소켓 파일은 있고 `inactive (dead)`. 연결한 뒤 `virtlogd`는 `active (running)` |
| 스왑(zram) | `swapon --show`와 `s6-rc -a list \| grep -x zram` | 우선순위 `100`인 `/dev/zram0`, 크기는 systemd 부팅과 같음(기본 `min(ram, 8 GiB)`). `zram`이 나옵니다 |
| 무선 상태(rfkill) | GNOME 설정에서 블루투스를 끄고 s6 항목으로 재부팅한 뒤 `rfkill`과 `s6-rc -a list \| grep -x rfkill` | 재부팅 뒤에도 블루투스가 `blocked`. `rfkill`이 나옵니다(systemd-rfkill이 아니라 oneshot) |
| reload 감시 | `sudo s6-svstat /run/service/uinitconverter-linkwatch`와 `sudo grep -h uinitconverter-linkwatch /run/uncaught-logs/current` | `up (pid …)`. 로그 줄은 reload가 멈췄을 때만 나옵니다(`… sent its start event`) |
| reload 감시가 기다림을 풀어 줌 | `sudo tests/linkwatch-check.sh` (30초쯤 걸립니다. /run/uinitconverter/lwtest에 임시 supervisor를 만들고 끝나면 지웁니다) | `ok:` 다섯 줄과 종료 코드 0: s6-rc-update라는 이름으로 기다리는 쪽이 10초쯤 뒤 놓친 시작 이벤트를 받고, 일반 s6-ftrig-wait는 시간 초과되며, linkwatch가 보낸 것을 로그에 남기고, 거부가 없습니다 |
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
| s6 세트 | `s6 set status gdm.service sshd.service dbus` | `gdm.service/active`, `sshd.service/usable`(sshd를 활성화하지 않았다면), `dbus/always` |
| 부팅할 것은 s6 세트가 정함 | `sudo s6 set disable -p chronyd.service`, `sudo systemctl daemon-reload`, `s6 set status chronyd.service`, s6로 재부팅, `systemctl is-active chronyd.service`. 그다음 `sudo s6 set enable chronyd.service`, `sudo systemctl daemon-reload` | reload 후 `chronyd.service/usable`과 `chronyd.service.log/usable`(`s6 set status chronyd.service chronyd.service.log`), `systemctl is-enabled chronyd.service`는 `disabled`. 재부팅 후 `inactive`. enable과 reload 후에는 둘 다 `active`, `enabled` |
| 셸에서 `s6 live install` | `sudo s6 set commit --no-snapshot && sudo s6 live install --no-snapshot`, 그다음 `ls -Z /run/service \| awk '{print $1}' \| sort \| uniq -c`와 `sudo ausearch -m AVC -ts recent` | 명령이 끝나고, 링크는 `tmpfs_t` 또는 `init_var_run_t`이며 `user_tmp_t`는 없고, 거부(denial)도 없음 |
| 이전 종료 | s6에서 전원을 끄고 다시 s6로 부팅한 뒤 `sudo ls -l /var/log/uinitconverter/last-shutdown-*.log; sudo grep -ci denied /var/log/uinitconverter/last-shutdown-*.log` | 두 파일 모두 전원을 끈 시각의 것이고 거부(denied)는 `0`입니다(아니면 파일을 보내 주세요). 전원 끄기가 25초 이상이 아니라 몇 초 걸렸고, 콘솔 로그가 비어 있지 않으며 `unable to rename`, `Failed to activate service ... timed out`, `uinitconverterd: stopping ... took`이 없고, 커널 로그에 `services still not down after 180 s`가 없습니다 |
| longrun의 ExecStop= | `printf '[Service]\nExecStart=/bin/sleep infinity\nExecStop=/bin/sh -c "echo stop $$MAINPID >> /var/tmp/sdc-stop.log"\n' \| sudo tee /etc/systemd/system/sdc-stoptest.service; sudo systemctl daemon-reload; sudo systemctl start sdc-stoptest; sleep 2; s6 live status sdc-stoptest.service.stop; sudo s6 live stop sdc-stoptest.service; cat /var/tmp/sdc-stop.log`. 그다음 `sudo systemctl start sdc-stoptest`, 전원을 끄고 다시 s6로 부팅해 `cat /var/tmp/sdc-stop.log`. 끝나면 `sudo rm /etc/systemd/system/sdc-stoptest.service /var/tmp/sdc-stop.log; sudo systemctl daemon-reload` | `sdc-stoptest.service.stop`이 올라와 있습니다. `s6 live stop` 뒤에 `stop PID` 줄이 하나, 전원을 끈 뒤 하나가 더 있습니다. 이전 종료 항목도 그대로 맞습니다 |
| 버스와 userdb 레이블 | `ls -dZ /run/dbus/system_bus_socket /run/systemd/userdb` | `system_dbusd_var_run_t`와 `systemd_userdbd_runtime_t` |
| catch-all fifo 레이블 | `ls -Z /run/service/s6-svscan-log/fifo`와 `sudo ausearch -m AVC -ts boot \| grep s6-svscan-log/fifo` | `uinitconverter_log_fifo_t`이고, 이 fifo에 대한 거부가 없음 (`initctl_t`이면 서비스의 stderr가 /dev/null로 바뀜) |
| 셸에서 기다리기 | 재부팅 후 `ls -dZ /run/service/s6-svscan-log/event /run/service/uinitconverterd/event`; `sudo s6-svc -wR -T 30000 -r /run/service/uinitconverterd`; `sudo ausearch -m AVC -ts recent \| grep ftrig` | 둘 다 `init_var_run_t`; s6-svc가 바로 0으로 끝남; 거부 없음 (`tmpfs_t`이면 기다리는 쪽의 fifo가 `user_tmp_t`가 되어 s6-supervise가 쓰지 못하고 기다림이 시간 초과됨) |
| 전체 상태 | `systemctl is-system-running` | `running` (`degraded`도 괜찮습니다. 다음 줄 참고) |
| 실패한 유닛 | `systemctl --failed` | 없는 것이 좋습니다. 있으면 적어 두세요 |
| 저널 | `journalctl -b \| head` | 이번 부팅의 로그 |
| status의 로그 | `systemctl status NetworkManager` | 서비스의 최근 로그 줄이 보입니다 |
| 로그인 세션 | `loginctl` | 내 사용자의 세션 |
| 사용자 매니저 | `systemctl --user status` | 사용자 서비스 트리 |
| 네트워크 | `ping -c 3 fedoraproject.org` | 응답이 옵니다 |
| resolved 없는 DNS(`--resolved=no`일 때만) | `pgrep -x systemd-resolve; cat /etc/resolv.conf` | PID 없음, `# Generated by NetworkManager`와 네트워크의 네임서버 |
| 시스템 버스(`--bus=broker`일 때만) | `s6-svstat /run/service/dbus; ps -o args= -p "$(s6-svstat -p /run/service/dbus)"; ls -Z /run/dbus/system_bus_socket; grep 'dbus:' /run/uncaught-logs/current` | up, `dbus-broker-launch --scope system --audit`, `system_dbusd_var_run_t`, "dbus-daemon instead of dbus-broker" 줄 없음 |
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

실행할 때마다 `~/uinitconverter-mem/mem-<s6|systemd>-<날짜>.tsv`가 생깁니다. 그다음
어느 부팅에서든:

```sh
tests/memcompare.sh compare ~/uinitconverter-mem/mem-systemd-*.tsv ~/uinitconverter-mem/mem-s6-*.tsv
```

시스템 전체 수치(`free`와 같은 방식), 그룹별 메모리(init과 감독, 로그, 사용자
매니저, 나머지), 메모리 차이가 가장 큰 명령들을 보여 줍니다. root로 실행하면
PSS(공유 페이지를 쓰는 프로세스들에 나눠 계산한 값)를 비교하고, root가 아니면
개인 익명 메모리로 비교합니다.

**또는 부팅할 때마다 스스로 측정하게 하세요.** 스냅샷 서비스를 한 번 설치합니다
(어느 부팅에서든):

```sh
sudo tests/memtest.sh install        # --wait 초(600), --max-diff MiB(50)
```

그다음에는 systemd 항목과 s6 항목으로 원하는 만큼 재부팅하기만 하면 됩니다.
부팅할 때마다 LUKS 암호를 입력하고 10분 동안 로그인하지 마세요. 서비스는 시작한
뒤 600초를 기다리고(암호 입력 화면에서 보낸 시간은 세지 않습니다), GDM 로그인
화면 상태에서 `~/uinitconverter-mem/memtest/`에 스냅샷을 찍고 그곳의
`report.txt`를 다시 씁니다.

```sh
tests/memtest.sh report              # 같은 보고서를 언제든
```

보고서에는 init별 스냅샷의 중앙값(각각 몇 번씩 부팅하면 잡음이 줄어듭니다),
스냅샷마다 한 줄, 가장 최근 한 쌍의 `memcompare.sh compare` 상세 내용이 있고,
s6 부팅의 "total - available"이 systemd 부팅보다 50 MiB 이하로 많으면
`Result: PASS`입니다(종료 코드 0, `FAIL`은 1, 두 init 모두 스냅샷이 생기기
전에는 `INCOMPLETE`로 2). 처음부터 다시 하려면 이전 `.tsv` 파일을 지우세요.
`sudo tests/memtest.sh run`은 손으로 스냅샷을 찍고, `sudo tests/memtest.sh
uninstall`은 서비스를 제거합니다(스냅샷은 남습니다). 서비스 자신의 프로세스는
스냅샷에서 빠지며, 로그는 `/var/lib/uinitconverter-memtest/log`입니다.
`tests/memtest-check.sh`는 스크립트 자체를 root 없이 검사합니다.

## s6 부팅이 실패할 때

1. **Ctrl+Alt+F9**를 누르세요. 나머지 부팅이 실패해도 그곳에서는 복구용 로그인
   프롬프트가 실행됩니다. root로 로그인한 뒤:
   ```sh
   tail -50 /run/uncaught-logs/current      # s6와 서비스가 출력한 내용
   s6-rc -a list                            # 실행 중인 것
   ```
2. 최신 s6 데이터베이스가 문제라면, 재부팅해서 GRUB의 s6 항목에서 **e**를 누르고
   `linux` 줄에 다음 중 하나를 추가하세요.
   - `uinitconverter.db=previous`는 마지막 것 이전에 설치된 데이터베이스로 부팅합니다.
3. 빠져나오려면 재부팅해서 일반 Fedora 항목을 고르세요. journald가 실행됐었다면 실패한
   s6 부팅의 로그를 `journalctl -b -1`로 볼 수 있습니다. uinitconverter 자신의 메시지(데몬,
   그리고 PAM 세션이나 exec 실패 같은 러너의 오류)도 그곳에 남습니다:
   `journalctl -b -1 -t uinitconverterd -t uinitconverter`.
4. 들어갈 수 있는 부팅에서 `sudo tests/collect-diagnostics.sh`를 실행하고 압축 파일을
   보내 주세요. 복구 콘솔만 쓸 수 있다면 `/run/uncaught-logs/current`를 보내 주세요.

VM 스냅샷으로 되돌리면 모든 변경이 사라집니다.

## 새 버전으로 업데이트하기

```sh
cd useless-init-converter
git pull
packages/build-rpms.sh
for b in standalone/*/build-rpm.sh; do "$b"; done
sudo dnf upgrade ~/rpmbuild-uinitconverter/rpms/*.rpm ~/rpmbuild-uinitconverter-standalone/rpms/*.rpm
sudo uinitconverter init --yes
```

(GitHub 릴리스에서는 다음 릴리스의 파일을 받아 `sudo dnf upgrade ./*.rpm` 다음 `sudo uinitconverter init --yes`.)

프로젝트 이름이 바뀌기 전의 설치(sdcompat 패키지, `/etc/sdcompat`)는 이렇게 업데이트할 수
없습니다. [packages/README.md](packages/README.md#sdcompat에서-옮겨-오기)의 "sdcompat에서
옮겨 오기"대로 지운 뒤 uinitconverter를 새로 설치하세요. 예전 클론은 `git pull`로 계속 쓸 수
있고(GitHub이 예전 저장소 이름을 새 이름으로 넘겨 줍니다), 디렉터리 이름만 예전 그대로입니다.

`uinitconverter init`을 다시 실행해도 안전합니다. `/etc/uinitconverter` 설정과 지난 실행의 선택은
그대로 두고, 데이터베이스, s6-linux-init 트리, 부팅 항목을 다시 만들며, 바꾸는 파일은
백업합니다. `/etc/uinitconverter/s6`의 s6 서비스는 패키지 소유입니다. dnf가 고치지 않은 것은
새 버전으로 바꾸고, 고친 파일은 그대로 두며 새 버전을 옆에 `파일.rpmnew`로 남기니 직접
합치세요. s6로 부팅한 상태에서 실행하면 init이 uinitconverterd를 재시작해서 재부팅 없이 새
바이너리가 동작합니다(uinitconverter-fdstore와 journal-relay는 다음 부팅까지 이전 바이너리를
씁니다). SELinux 정책 모듈도 다시 불러옵니다. 불러와졌는지 확인하세요.

```sh
sudo semodule -l | grep -x uinitconverter
```

업데이트에서 `dist/skarnet.versions`나 `dist/rpm/`이 바뀌었다면 먼저 s6 패키지를 다시 빌드해
설치하세요(2단계). 이 작업은 **일반 systemd 부팅**에서 하세요. 그러면 새 s6, s6-rc 프로그램이
이전 버전이 띄운 s6 트리를 건드리지 않고, 다음 s6 부팅에서 새 s6-rc로 데이터베이스를
컴파일합니다.

## s6 부팅을 기본값으로 만들기

s6 부팅이 잘 되면(위의 확인 항목을 통과하고, 가능하면 enforcing으로) 그 항목을 기본값으로
만드세요(init은 다른 선택을 유지합니다).

```sh
sudo uinitconverter init --selinux=enforcing --default-s6
```

확인:

| 항목 | 명령 | 기대 결과 |
|---|---|---|
| 기본값 | `sudo grubby --info=DEFAULT \| grep title` | `title="Fedora (s6 + uinitconverter) <커널>"` |
| 선택이 유지됨 | `cat /etc/uinitconverter/boot-default` | `s6` |
| 부팅됨 | 메뉴를 건드리지 않고 재부팅, `cat /proc/1/comm` | `s6-svscan` |
| 커널 업데이트 후 | `sudo dnf upgrade kernel*`, 그다음 첫 줄을 다시 | 새 커널의 s6 항목 |

systemd 항목으로 한 번만 부팅하려면 GRUB이 시작할 때 Shift를 누르고 있거나 Esc를 눌러 고르세요.
systemd를 다시 기본값으로 하려면 `sudo uinitconverter init --default-systemd`를 실행하세요.

## systemd 없이 부팅하기

`--boot-only-s6=yes`는 s6 항목에 uinitconverter의 initramfs를 줍니다. s6-svscan이 그 PID 1이고 mdevd가
드라이버를 로드하므로, initramfs에서도 systemd가 실행되지 않습니다. LUKS 루트는 암호를 입력해
열어야 합니다.

먼저 머신을 건드리지 않고 QEMU에서 시험하세요(mdevd가 설치되어 있거나, `mdevd`와
`mdevd-coldplug`가 있는 디렉터리를 `MDEVD_BIN=DIR`로 주어야 합니다. root는 필요 없습니다):

```sh
tests/initramfs.sh /tmp/sdc-initramfs
```

btrfs 루트를 `subvol=root`에 둔 LUKS2 디스크를 만들고, 실행 중인 커널을 s6 initramfs로 부팅해
시리얼 콘솔에서 암호를 입력합니다. 기대 결과: `passed: 6  failed: 0`(암호 입력, udev 링크
없는 `root=/dev/disk/by-uuid/…`, mdevd가 드라이버를 로드해야 하는 NVMe 디스크, 틀린 암호 세 번 뒤 복구 셸과 재시도, `rd.break`, 디스크
없음).

그다음 머신에서:

```sh
sudo uinitconverter init --boot-only-s6=yes
```

| 항목 | 명령 | 기대 결과 |
|---|---|---|
| 이미지 | `ls -l /boot/initramfs-*-s6.img` | 설치된 커널마다 하나, 방금 만들어짐 |
| 항목이 그것을 씀 | `sudo sh -c 'grep ^initrd /boot/loader/entries/*~uinitconverter.conf'` | `initramfs-<커널>-s6.img`. systemd 항목은 `initramfs-<커널>.img` 그대로 |
| 암호 입력 | s6 항목으로 부팅 | 평범한 `Enter passphrase for /dev/nvme…:` 프롬프트, 입력 뒤 부팅이 계속됨 |
| systemd가 실행되지 않음 | `sudo dmesg \| grep -E 'Run /init\|systemd\[1\]\|running in initrd'` | `Run /init as init process`만 나옴 |
| PID 1 | `cat /proc/1/comm` | `s6-svscan` |
| LUKS 매핑의 링크 | `ls -l /dev/disk/by-uuid/ \| grep dm-`와 `udevadm info /dev/dm-0 \| grep -E 'DM_NAME\|ID_FS_TYPE'` | 루트 파일 시스템의 UUID가 `../../dm-0`을 가리킴. `DM_NAME=luks-…`, `ID_FS_TYPE=btrfs` |
| 마운트 | `findmnt -n /; findmnt -n /home` | 둘 다 전처럼 LUKS 매핑에서 |
| 복구(선택) | 틀린 암호를 세 번 입력 | `initramfs: rescue shell …`. `exit`하면 다시 물음 |
| 커널 업데이트 | `sudo dnf upgrade kernel*` 뒤 처음 두 줄 | 새 커널에 `-s6.img`가 있고 그 s6 항목이 그것을 씀 |

암호 프롬프트가 나오지 않거나 디스크를 찾지 못하면 콘솔에 복구 셸이 열립니다. `dmesg`, `ls /dev`,
`cat /proc/cmdline`으로 무엇이 없는지 보세요(결과를 보내 주세요). Fedora의 initramfs로 한 번만
부팅하려면 s6 항목에서 `e`를 눌러 `initramfs-<커널>-s6.img`를 `initramfs-<커널>.img`로 바꾸세요.
완전히 되돌리려면 `sudo uinitconverter init --boot-only-s6=no`를 실행하세요.

## 제거하기

일반 Fedora 항목으로 부팅한 뒤:

```sh
sudo uinitconverter init --uninstall
sudo dnf remove uinitconverter uinitconverter-gen uinitconverter-runner uinitconverter-timers uinitconverterd \
    uinitconverter-fdstore uinitconverter-notify uinitconverter-journal-relay uinitconverter-linkwatch
sudo dnf remove skalibs execline s6 s6-rc s6-linux-init s6-frontend mdevd \
    skalibs-devel execline-devel s6-devel s6-rc-devel s6-linux-init-devel
```

`/etc/uinitconverter`, `/var/lib/uinitconverter`, 활성화한 유닛은 남습니다. systemd 부팅에서는 아무
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
