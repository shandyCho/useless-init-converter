# uinitconverter RPM 패키지

[English](README-us.md)

본체(`crates/`)의 실행 파일은 각각 RPM 패키지 하나가 됩니다. 소스는 Cargo 작업공간
하나에 그대로 있고, 다섯 패키지 모두 같은 소스 타르볼로 빌드합니다. 라이브러리
크레이트(`uinitconverter-base`, `uinitconverter-install`, `uinitconverter-exec`, `uinitconverter-client`)는
실행 파일에 정적으로 링크되므로 빌드할 때만 쓰이고, 따로 패키지가 되지 않습니다.

| 패키지 | 들어 있는 것 | 실행할 때 필요한 패키지 (`Requires`) |
|---|---|---|
| `uinitconverter-runner` | `uinitconverter-runner` | s6, shadow-utils. `uinitconverterd`는 권장(`OnFailure=`/`OnSuccess=` 유닛을 데몬으로 시작) |
| `uinitconverterd` | `uinitconverterd`, D-Bus 정책(`uinitconverter.conf`) | s6, s6-rc, dbus-daemon, `uinitconverter-runner`. `uinitconverter-timers`는 권장(`systemd-run --on-calendar`) |
| `uinitconverter-timers` | `uinitconverter-timers` | `uinitconverterd` (타이머가 유닛을 데몬으로 시작) |
| `uinitconverter-gen` | `uinitconverter-gen`, `/var/lib/uinitconverter` | s6-rc, `uinitconverter-runner`, `uinitconverter-timers` (생성된 서비스가 실행) |
| `uinitconverter` | `uinitconverter` 명령(부팅 설정 `uinitconverter init` 포함), `early`, 설정, s6 서비스, s6-linux-init 골격, SELinux 모듈, kernel-install 플러그인, `/var/log/uinitconverter` | s6, s6-linux-init, s6-rc, execline, dbus-daemon, util-linux, policycoreutils-python-utils, grubby, 그리고 위의 네 패키지 |

- `dnf install uinitconverter`이면 전부 설치됩니다. 부팅에 필요한 설정과 SELinux 모듈은
  `uinitconverter` 패키지에 있으므로, s6로 부팅하려면 이 패키지가 있어야 합니다.
- 도구 패키지는 따로 업데이트할 수 있습니다. 같은 버전끼리만 맞도록
  (`= 버전-릴리스`) 묶여 있습니다.
- 독립 도구(`standalone/`)는 여기에 없습니다. 각 디렉터리에 자기 spec과
  `build-rpm.sh`(결과: `~/rpmbuild-uinitconverter-standalone/rpms`)가 있고,
  `uinitconverter`은 이들을 `Recommends`/`Suggests`로만 가리킵니다.

## 빌드

```sh
packages/build-rpms.sh            # 결과: ~/rpmbuild-uinitconverter/rpms
packages/build-rpms.sh DIR        # 다른 위치
packages/build-rpms.sh --srpm     # 소스 RPM만: ~/rpmbuild-uinitconverter/srpms
```

스크립트는 작업공간(`Cargo.toml`, `Cargo.lock`, `crates`, `dist`)과 `cargo vendor`로
받은 크레이트를 `uinitconverter-VERSION.tar.gz` 하나로 묶고, spec마다 `rpmbuild -bb`를
실행합니다. rpmbuild는 네트워크 없이(`--offline`) 빌드합니다. 아무것도 설치하지 않고
sudo도 쓰지 않습니다. 필요한 것: `rpm-build`, `cargo`, `rust`.

## 버전

spec에는 버전을 적지 않습니다. 스크립트가 정해서 넘겨 주므로 spec은 이 스크립트로
빌드하세요(`rpmbuild`를 직접 실행하면 버전이 없어 실패합니다).

- **버전**은 `Cargo.toml`의 `[workspace.package] version` 하나에서 읽습니다. 크레이트들도
  이 값을 쓰므로, 버전을 올릴 때는 이 줄만 고치면 됩니다.
- **릴리스**는 빌드할 때마다 새로 정합니다: `1.YYYYMMDDHHMMSS.g커밋`. 커밋하지 않은
  변경이 있으면 끝에 `.dirty`가 붙습니다. 예: `uinitconverter-gen-0.1.0-1.20261021071849.g0023111.fc45`.
  빌드 시각이 앞에 오므로, 버전을 그대로 두고 Rust 코드만 고쳐서 다시 빌드해도 dnf는
  새 빌드를 업그레이드로 봅니다.
- 다섯 패키지는 같은 버전-릴리스끼리만 맞도록 묶여 있고, 스크립트는 언제나 다섯 개를
  함께 빌드합니다. `rpms/`에는 마지막 빌드의 다섯 개만 남습니다.
- RPM으로 빌드한 도구의 `--version`은 패키지의 버전-릴리스를 보여 줍니다(예:
  `uinitconverter-gen 0.1.0-1.20261021073517.gc8aabf8.fc45`). 소스에서 빌드하면
  `0.1.0 (git c8aabf8)`처럼 커밋을 보여 줍니다.
- 공개 릴리스처럼 릴리스 번호를 직접 정하려면 `UINITCONVERTER_RELEASE=1 packages/build-rpms.sh`.

## GitHub 릴리스

패키지는 프로젝트의 [GitHub 릴리스](https://github.com/shandyCho/useless-init-converter/releases)에
파일로 올립니다.

```sh
packages/github-release.sh             # 결과: ~/uinitconverter-release
packages/github-release.sh DIR         # 다른 위치
```

커밋된 트리가 필요하고, 새로 만든 `DIR/work`에서 `packages/build-rpms.sh`와
`standalone/*/build-rpm.sh`를 실행한 뒤 다음을 모읍니다.

- `DIR/assets/`: 바이너리 RPM(본체 다섯 패키지와 독립 도구. debuginfo 패키지는 빼고)과
  `SHA256SUMS`. skarnet.org 패키지는 릴리스에 넣지 않습니다. 사용자가 각자
  `dist/rpm/build-skarnet-rpms.sh`로 빌드합니다.
- `DIR/NOTES.md`: 릴리스 본문. 커밋, RPM을 빌드한 Fedora 릴리스와 아키텍처, 설치 명령,
  패키지 목록이 들어 있습니다.

끝에 GitHub CLI가 로그인된 곳에서 실행할 `gh release create` 명령을 출력합니다(태그는
`[workspace.package] version`에서 만든 `vVERSION`). CLI 없이 하려면 GitHub에서
Releases > Draft a new release를 열고, 그 커밋에 태그 `vVERSION`을 달고, NOTES.md를
본문으로 넣고, `DIR/assets`의 파일을 모두 첨부합니다. 태그는 한 번만 쓸 수 있으므로
릴리스마다 `Cargo.toml`의 버전을 올리세요.

RPM은 빌드한 머신의 Fedora 릴리스와 아키텍처용입니다(예: `fc45`, `x86_64`). 지원할 Fedora
릴리스마다 그 위에서 빌드하세요. 사용자는 먼저 릴리스의 태그에서 skarnet.org 패키지를
빌드해 설치합니다.

```sh
git clone --branch vVERSION https://github.com/shandyCho/useless-init-converter.git
sudo dnf install git rpm-build gcc make cpio
useless-init-converter/dist/rpm/build-skarnet-rpms.sh            # 타르볼은 먼저 ~/skarnet-sources에 직접 받기(README.md 참고)
sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm
```

그다음 빈 디렉터리에서:

```sh
gh release download vVERSION -R shandyCho/useless-init-converter    # 또는 브라우저로 파일을 받기
sha256sum -c SHA256SUMS
sudo dnf install ./*.rpm
uinitconverter init --dry-run
sudo uinitconverter init
```

rsyslog, earlyoom 같은 다른 의존 패키지는 dnf가 Fedora 저장소에서 가져옵니다. GitHub에는
저장소가 없으므로 `dnf upgrade`만으로는 새 릴리스를 알 수 없습니다. 다음 릴리스의 파일을
받아 `sudo dnf upgrade ./*.rpm`을 실행한 뒤 `sudo uinitconverter init --yes`를 실행하세요. skarnet
패키지는 `dist/skarnet.versions`가 바뀔 때만 다시 빌드하면 됩니다.

spec에는 아직 `License: TBD`라고 되어 있습니다. 공개하기 전에 라이선스를 정하세요.

## 직접 빌드한 패키지 설치

```sh
sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm      # skarnet 도구 (dist/rpm/build-skarnet-rpms.sh)
sudo dnf install ~/rpmbuild-uinitconverter/rpms/*.rpm
sudo dnf install ~/rpmbuild-uinitconverter-standalone/rpms/*.rpm   # 선택 (standalone/*/build-rpm.sh)
sudo uinitconverter init
```

Rust 코드를 고친 뒤에는 다시 빌드하고 업그레이드합니다.

```sh
git pull                                            # 또는 직접 고치고 커밋
packages/build-rpms.sh
sudo dnf upgrade ~/rpmbuild-uinitconverter/rpms/*.rpm
uinitconverter --version                                  # 설치된 빌드 확인
```

s6로 부팅한 상태라면, 새 uinitconverterd는 데몬을 다시 시작하면 쓰입니다(`s6-svc -r
/run/service/uinitconverterd`). runner와 timers는 서비스가 다음에 시작될 때 쓰이고,
uinitconverter-gen은 다음 `uinitconverter reload` 때 쓰입니다(`systemctl daemon-reload`는 uinitconverterd에
들어 있는 사본을 쓰므로, 데몬이 다시 시작된 뒤에 바뀝니다). 부팅 때는 아무것도 다시 만들지
않습니다.

s6-rc 저장소가 없던 버전(`s6 set` 변경 이전)에서 옮겨 올 때는 `sudo uinitconverter init`을 다시
실행하세요. `/etc/uinitconverter/s6/default`를 지우고(이제 세트의 커밋이 `default` 번들을 만듭니다.
이것이 있으면 reload가 실행을 거부합니다), 새 `/etc/s6.conf`를 쓰고, `/etc/s6/sources`를
만들고, 새 SELinux 모듈을 불러오고, systemd의 활성화 상태에서 시작하는 세트 `current`와 함께
저장소를 만듭니다.

s6-linux-init 스켈레톤(`rc.init`, `rc.shutdown`, `rc.shutdown.final`)은
다릅니다. 패키지는 init이 쓰는 원본을 설치할 뿐이고, 부팅에 쓰이는 것은
`s6-linux-init-maker`가 그것으로 만든 `/etc/s6-linux-init/current`입니다.
업그레이드로 스켈레톤이 바뀌면 `sudo uinitconverter init`을 다시 실행해(지난 실행의 선택을
유지합니다) 새로 만드세요. `early`와 나머지는 그러지 않아도 다음 부팅부터 적용됩니다.

### sdcompat에서 옮겨 오기

이 프로젝트의 예전 이름은 sdcompat입니다. 모든 이름이 바뀌었습니다(명령, 패키지,
`/etc/sdcompat` → `/etc/uinitconverter`, `/var/lib/sdcompat`, SELinux 모듈, D-Bus
인터페이스, `sdcompat.db=` → `uinitconverter.db=`, GRUB 항목). 자동 이전은 없습니다.
sdcompat을 지운 뒤 uinitconverter를 새로 설치하세요. **일반 systemd 부팅**에서:

```sh
sudo cp -a /etc/sdcompat ~/sdcompat-etc     # 설정. 나중에 직접 옮깁니다
cat /etc/sdcompat/boot-options              # 지난 init 실행의 선택
sudo sdcompat init --uninstall              # s6 부팅 항목, initramfs, s6-linux-init 트리
sudo dnf remove sdcompat sdcompat-gen sdcompat-runner sdcompat-timers sdcompatd \
    sdcompat-fdstore sdcompat-journal-relay sdcompat-linkwatch sdcompat-notify
sudo mv /var/lib/s6-rc/repository /var/lib/s6-rc/repository.sdcompat
sudo rm -rf /var/lib/sdcompat /var/log/sdcompat
```

패키지를 지우면 예전 SELinux 모듈도 내려갑니다. s6-rc 저장소는 세트가 아직 예전 서비스
이름을 담고 있으므로 옆으로 옮깁니다. `uinitconverter init`이 systemd의 활성화 상태에서 새
저장소를 만듭니다(직접 바꾼 `s6 set` 변경은 다시 해야 합니다). 그다음 uinitconverter RPM을
설치하고 예전에 쓰던 옵션으로 `sudo uinitconverter init`을 실행하세요(예: `--journal=no
--boot-only-s6=yes --selinux=enforcing`. 예전 선택은 읽지 않습니다). 고친 설정은
`~/sdcompat-etc`에서 `/etc/uinitconverter`로 직접 옮기세요(파일 이름은 같고
`sdcompat.conf`만 `uinitconverter.conf`가 됩니다).

제거: `sudo uinitconverter init --uninstall` 다음
`dnf remove uinitconverter uinitconverter-gen uinitconverter-runner uinitconverter-timers uinitconverterd`.
