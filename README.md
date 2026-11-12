# useless-init-converter

> **이 소프트웨어는 AI가 생성한 저품질 코드(AI slop)이며, 설치와 실행에 따른 책임은 사용자에게 있습니다.**

[English](README-us.md) | **한국어**

## 프로젝트 요약

uinitconverter는 systemd 대신 **s6를 PID 1로** 삼아 Fedora를 실행합니다. Fedora의 유닛
파일을 s6-rc 서비스로 변환하고, 관리는 s6 자체의 도구로 합니다. `systemctl`, libsystemd,
D-Bus 클라이언트가 계속 동작하므로 dnf, SELinux enforcing, GNOME도 그대로 쓸 수 있습니다.
패키지와 경로의 이름은 `uinitconverter`이고, 명령은 `initconvert`(`initconvert init`, `initconvert reload` 등)와
`initconvertd`, `initconvert-gen`, `initconvert-runner` 등입니다.

**설치 전에 읽어 주세요.**

- **얻는 것은 속도나 메모리가 아니라 제어권입니다.** s6를 꼭 쓰려는 이유가 없다면 일반
  systemd가 더 나은 선택입니다.
- **실제 부팅은 Fedora 45 Workstation 한 대에서만 확인했습니다.** 먼저 가상 머신에서
  시도하세요.
- **빠진 systemd 기능이 있습니다.** 제너레이터, 자격 증명 등입니다.
  [아직 구현되지 않은 기능](docs/etc.md#아직-구현되지-않은-기능)을 보세요.
- **기존 systemd 부팅은 그대로 남습니다.** 설치하면 GRUB 항목이 추가될 뿐이고, 직접
  바꾸기 전까지 기본 부팅은 systemd입니다.

### 설치되는 것

| 묶음 | 패키지 | 필요 여부 | 받는 곳 |
|---|---|---|---|
| s6와 관련 도구 | skalibs, execline, s6, s6-rc, s6-linux-init, s6-frontend, mdevd | 필수 | 직접 빌드(2단계) |
| TCP 슈퍼서버 | s6-dns, s6-networking | 선택: `sshd.socket` 같은 TCP `Accept=yes` 소켓에만 필요 | 2단계에서 빌드. 빼려면 `dnf install` 전에 그 `.rpm` 파일을 지우세요 |
| 본체 | `uinitconverter`, `uinitconverterd`, `uinitconverter-gen`, `uinitconverter-runner`, `uinitconverter-timers`, `uinitconverter-devices` | 필수 | 릴리스 또는 소스(3단계) |
| 독립 도구 | 아래 네 가지 | 선택 | 릴리스 또는 소스(3단계) |

독립 도구는 별도 프로젝트입니다. 각각 따로 빌드, 설치, 실행할 수 있고 본체는 이들 없이도
동작하지만, 마지막 열의 내용을 감수해야 합니다.

| 도구 | 하는 일 | 없을 때 |
|---|---|---|
| [`uinitconverter-linkwatch`](standalone/uinitconverter-linkwatch/README.md) | s6 경쟁 상태로 멈춘 daemon-reload나 그 직후의 시작을 풀어 줍니다 | 멈춘 `s6-rc-update`를 죽일 때까지 그 reload와 이후의 모든 s6-rc 명령이 멈춥니다. 멈춘 시작은 유닛의 시작 타임아웃까지 s6-rc의 잠금을 잡고 있습니다 |
| [`uinitconverter-fdstore`](standalone/uinitconverter-fdstore/README.md) | 서비스가 저장한 파일 디스크립터와 수신 소켓을 보관합니다 | `initconvertd`가 재시작할 때 사라지고, 요청 시 시작하는 서비스가 부팅 때 시작됩니다 |
| [`uinitconverter-notify`](standalone/uinitconverter-notify/README.md) | `systemd-notify` 대체 도구입니다 | 프로그램들이 계속 설치되어 있는 `systemd-notify`를 씁니다 |
| [`uinitconverter-journal-relay`](standalone/uinitconverter-journal-relay/README.md) | journald가 없을 때(기본값) journald의 클라이언트 소켓을 대신합니다 | `--journal=yes`(journald)만 쓸 수 있습니다 |

네 가지를 모두 설치하는 것이 가장 간단하며, 3단계도 그렇게 합니다.

## 필요한 것

**머신**

- x86_64의 Fedora 45 Workstation. 릴리스 RPM은 이 환경용으로 빌드되어 있습니다. 다른
  Fedora 릴리스나 아키텍처라면 소스에서 빌드하세요(3단계의 방법 B). 다만 실제 부팅은
  Fedora 45에서만 확인했습니다.
- 메모리 4GB, 디스크 30GB 이상의 가상 머신을 권장합니다.
- `sudo`를 쓸 수 있는 사용자와 인터넷 연결.

**빌드와 설치에 필요한 것**

| 용도 | Fedora 패키지 |
|---|---|
| 1, 2단계(항상) | `git curl rpm-build gcc make cpio` |
| 3단계의 방법 B(소스 빌드 때만) | `cargo rust` |

**실행에 필요한 것**

- 2단계의 s6 패키지. Fedora에는 없습니다.
- `systemd`는 지우지 말고 그대로 두어야 합니다. udevd, logind, journald,
  `systemd --user`, `systemctl`을 여전히 여기서 가져옵니다.
- `pam`, `libselinux`, `libseccomp`. Fedora Workstation에는 이미 있으며, `pam`이 없으면
  GNOME이 시작되지 않습니다.
- 나머지(dbus-daemon, grubby, earlyoom 등)는 dnf가 설치합니다.

전체 목록: [의존성](docs/etc.md#의존성)

## 설치

아래 명령을 한 터미널에서 순서대로 실행하세요(3단계에서 1단계의 `VERSION`을 씁니다).
1~3단계는 패키지만 설치하며, 4단계 전까지 부팅 메뉴는 바뀌지 않습니다.

### 0. 준비

가상 머신이라면 스냅샷을 만드세요. 그다음 복구 콘솔에 필요한 root 비밀번호를 설정하고,
부팅 항목을 고를 수 있도록 GRUB 메뉴가 항상 보이게 합니다.

```sh
sudo passwd root
sudo grub2-editenv - unset menu_auto_hide
```

### 1. 소스 받기

설치할 릴리스를 `VERSION`에 넣으세요.
[릴리스 목록](https://github.com/shandyCho/useless-init-converter/releases)에서 확인할 수 있습니다.

```sh
sudo dnf install -y git curl rpm-build gcc make cpio
VERSION=0.4.0
git clone --branch "v$VERSION" https://github.com/shandyCho/useless-init-converter.git
cd useless-init-converter
```

### 2. s6 패키지 빌드와 설치

`dist/skarnet.versions`에 적힌 버전을 정확히 내려받습니다. 빌드 스크립트가 각 파일의
SHA-256을 검사하고, 값이 다르면 중단합니다. 스크립트 자체는 아무것도 내려받지 않으며,
`--sources DIR`을 주지 않으면 `~/skarnet-sources`를 읽습니다.

```sh
mkdir -p ~/skarnet-sources
grep -v '^#' dist/skarnet.versions | while read -r name ver _; do
    [ -n "$name" ] || continue
    curl -fL -o ~/skarnet-sources/"$name-$ver.tar.gz" \
        "https://skarnet.org/software/$name/$name-$ver.tar.gz"
done
dist/rpm/build-skarnet-rpms.sh
sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm
```

### 3. uinitconverter와 독립 도구 설치

하나를 고르세요. 어느 쪽이든 본체와 독립 도구 네 가지를 모두 설치합니다. 빼고 싶은 독립
도구가 있으면 `dnf install` 줄을 실행하기 전에 그 `.rpm` 파일을 지우세요.

**방법 A: 릴리스에서 설치 (Fedora 45, x86_64)**

```sh
mkdir ~/uinitconverter-rpms && cd ~/uinitconverter-rpms
curl -fsSL "https://api.github.com/repos/shandyCho/useless-init-converter/releases/tags/v$VERSION" \
    | grep browser_download_url | cut -d'"' -f4 | xargs -n1 curl -fLO
sha256sum -c SHA256SUMS
sudo dnf install ./*.rpm
```

**방법 B: 소스에서 빌드**

```sh
sudo dnf install -y cargo rust
packages/build-rpms.sh
for script in standalone/*/build-rpm.sh; do "$script"; done
sudo dnf install ~/rpmbuild-uinitconverter/rpms/*.rpm \
    ~/rpmbuild-uinitconverter-standalone/rpms/*.rpm
```

### 4. 부팅 설정

첫 번째 명령은 무엇을 할지 보여 주기만 합니다. 두 번째 명령은 시작 전에 확인을 받은 뒤 GRUB
항목 "Fedora (s6 + uinitconverter)"를 추가합니다. 없는 패키지가 있으면 중단하고 `dnf install`
명령을 알려 줍니다.

```sh
initconvert init --dry-run
sudo initconvert init
```

첫 실행에서는 s6 부팅의 SELinux가 permissive 모드가 됩니다. 자주 쓰는 옵션은 아래와 같고,
전체 옵션은 [`initconvert init`이 하는 일](docs/etc.md#initconvert-init이-하는-일)에 있습니다.

| 옵션 | 효과 |
|---|---|
| `--selinux=enforcing` | s6 부팅의 SELinux를 enforcing으로 |
| `--default-s6` | s6를 기본 부팅 항목으로 |
| `--journal=yes` | 기본값인 rsyslog와 일반 텍스트 로그 대신 systemd-journald 실행(`journalctl`, `systemctl status`의 로그) |
| `--boot-only-s6=no` | Fedora의 initramfs 유지(기본값 `auto`는 루트를 부팅할 수 있으면 uinitconverter의 것을 써서 initramfs에서도 systemd를 쓰지 않음) |

### 5. s6로 부팅

```sh
sudo reboot
```

GRUB 메뉴에서 **Fedora (s6 + uinitconverter)**를 고르세요.

### 6. 확인

로그인한 뒤 실행합니다.

```sh
cat /proc/1/comm                  # s6-svscan이 나와야 합니다
initconvert --version          # 본체 도구가 모두 한 빌드인지 확인
s6 live status | head             # s6가 관리하는 서비스
sudo ausearch -m AVC -ts boot     # 이번 부팅의 SELinux 거부 기록
```

문제가 될 거부 기록이 없다면 다음 부팅부터 enforcing으로 바꾸세요.

```sh
sudo initconvert init --selinux=enforcing
```

### 부팅에 실패하면

재부팅해서 메뉴에 그대로 있는 일반 Fedora 항목을 고르세요. s6 부팅이 시작은
했지만 끝나지 않으면 Ctrl+Alt+F9를 누르고 복구 콘솔에 root로 로그인해
`/run/uncaught-logs/current`를 읽으세요. 문제를 보고하는 방법은 [TESTING.md](TESTING.md)에
있습니다.

### 업데이트

방법 A라면 새 릴리스를 3단계처럼 새 빈 디렉터리에 받은 뒤 실행합니다.

```sh
sudo dnf upgrade ./*.rpm
sudo initconvert init --yes
```

방법 B라면 새 소스를 받아 3단계의 빌드 명령 두 개를 다시 실행한 뒤 실행합니다.

```sh
sudo dnf upgrade ~/rpmbuild-uinitconverter/rpms/*.rpm \
    ~/rpmbuild-uinitconverter-standalone/rpms/*.rpm
sudo initconvert init --yes
```

s6 부팅에서는 `init`이 `initconvertd`를 다시 시작해 새 빌드로 바꿉니다. 업데이트를 마치려면 재부팅하세요.
s6 패키지는 `dist/skarnet.versions`가 바뀌었을 때만 다시 빌드합니다.

### 제거

일반 systemd 부팅에서 실행하세요.

```sh
sudo initconvert init --uninstall
sudo dnf remove 'uinitconverter*'
```

## 더 보기

- [docs/etc.md](docs/etc.md): 동작 방식, 구현된 기능, `s6` 명령, 설정, 소스 구조
- [TESTING.md](TESTING.md): 새로 클론하는 것부터 GNOME 세션까지의 단계별 테스트
- [packages/README.md](packages/README.md): RPM 패키지, 프로젝트의 옛 이름 sdcompat에서
  이전하기
