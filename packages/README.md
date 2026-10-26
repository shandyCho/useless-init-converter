# sdcompat RPM 패키지

[English](README-us.md)

본체(`crates/`)의 실행 파일은 각각 RPM 패키지 하나가 됩니다. 소스는 Cargo 작업공간
하나에 그대로 있고, 다섯 패키지 모두 같은 소스 타르볼로 빌드합니다. 라이브러리
크레이트(`sdcompat-base`, `sdcompat-install`, `sdcompat-exec`, `sdcompat-client`)는
실행 파일에 정적으로 링크되므로 빌드할 때만 쓰이고, 따로 패키지가 되지 않습니다.

| 패키지 | 들어 있는 것 | 실행할 때 필요한 패키지 (`Requires`) |
|---|---|---|
| `sdcompat-runner` | `sdcompat-runner` | s6, shadow-utils. `sdcompatd`는 권장(`OnFailure=`/`OnSuccess=` 유닛을 데몬으로 시작) |
| `sdcompatd` | `sdcompatd`, D-Bus 정책(`sdcompat.conf`) | s6, s6-rc, dbus-daemon, `sdcompat-runner`. `sdcompat-timers`는 권장(`systemd-run --on-calendar`) |
| `sdcompat-timers` | `sdcompat-timers` | `sdcompatd` (타이머가 유닛을 데몬으로 시작) |
| `sdcompat-gen` | `sdcompat-gen`, `/var/lib/sdcompat` | s6-rc, `sdcompat-runner`, `sdcompat-timers` (생성된 서비스가 실행) |
| `sdcompat` | `sdcompat` 명령(부팅 설정 `sdcompat init` 포함), `early`, 설정, s6 서비스, s6-linux-init 골격, SELinux 모듈, kernel-install 플러그인, `/var/log/sdcompat` | s6, s6-linux-init, s6-rc, execline, dbus-daemon, util-linux, policycoreutils-python-utils, grubby, 그리고 위의 네 패키지 |

- `dnf install sdcompat`이면 전부 설치됩니다. 부팅에 필요한 설정과 SELinux 모듈은
  `sdcompat` 패키지에 있으므로, s6로 부팅하려면 이 패키지가 있어야 합니다.
- 도구 패키지는 따로 업데이트할 수 있습니다. 같은 버전끼리만 맞도록
  (`= 버전-릴리스`) 묶여 있습니다.
- 독립 도구(`standalone/`)는 여기에 없습니다. 각 디렉터리에 자기 spec과
  `build-rpm.sh`(결과: `~/rpmbuild-sdcompat-standalone/rpms`)가 있고,
  `sdcompat`은 이들을 `Recommends`/`Suggests`로만 가리킵니다.

## 빌드

```sh
packages/build-rpms.sh            # 결과: ~/rpmbuild-sdcompat/rpms
packages/build-rpms.sh DIR        # 다른 위치
packages/build-rpms.sh --srpm     # 소스 RPM만: ~/rpmbuild-sdcompat/srpms (COPR 참고)
```

스크립트는 작업공간(`Cargo.toml`, `Cargo.lock`, `crates`, `dist`)과 `cargo vendor`로
받은 크레이트를 `sdcompat-VERSION.tar.gz` 하나로 묶고, spec마다 `rpmbuild -bb`를
실행합니다. rpmbuild는 네트워크 없이(`--offline`) 빌드합니다. 아무것도 설치하지 않고
sudo도 쓰지 않습니다. 필요한 것: `rpm-build`, `cargo`, `rust`.

## 버전

spec에는 버전을 적지 않습니다. 스크립트가 정해서 넘겨 주므로 spec은 이 스크립트로
빌드하세요(`rpmbuild`를 직접 실행하면 버전이 없어 실패합니다).

- **버전**은 `Cargo.toml`의 `[workspace.package] version` 하나에서 읽습니다. 크레이트들도
  이 값을 쓰므로, 버전을 올릴 때는 이 줄만 고치면 됩니다.
- **릴리스**는 빌드할 때마다 새로 정합니다: `1.YYYYMMDDHHMMSS.g커밋`. 커밋하지 않은
  변경이 있으면 끝에 `.dirty`가 붙습니다. 예: `sdcompat-gen-0.1.0-1.20261021071849.g0023111.fc45`.
  빌드 시각이 앞에 오므로, 버전을 그대로 두고 Rust 코드만 고쳐서 다시 빌드해도 dnf는
  새 빌드를 업그레이드로 봅니다.
- 다섯 패키지는 같은 버전-릴리스끼리만 맞도록 묶여 있고, 스크립트는 언제나 다섯 개를
  함께 빌드합니다. `rpms/`에는 마지막 빌드의 다섯 개만 남습니다.
- RPM으로 빌드한 도구의 `--version`은 패키지의 버전-릴리스를 보여 줍니다(예:
  `sdcompat-gen 0.1.0-1.20261021073517.gc8aabf8.fc45`). 소스에서 빌드하면
  `0.1.0 (git c8aabf8)`처럼 커밋을 보여 줍니다.
- 공개 릴리스처럼 릴리스 번호를 직접 정하려면 `SDCOMPAT_RELEASE=1 packages/build-rpms.sh`.

## COPR

패키지는 [Fedora COPR](https://copr.fedorainfracloud.org)에서 소스 RPM으로 빌드할 수
있습니다. 그러면 사용자는 dnf로 설치합니다.

```sh
packages/copr-srpms.sh                 # 소스 RPM 16개 전부: ~/sdcompat-srpms
packages/copr-srpms.sh core            # 본체 다섯 개만(코드를 고친 뒤)
packages/copr-srpms.sh DIR core standalone
```

각 빌드 스크립트를 `--srpm`으로 실행합니다(`packages/build-rpms.sh --srpm`,
`standalone/*/build-rpm.sh --srpm`, `dist/rpm/build-skarnet-rpms.sh --srpm`). 버전과
릴리스를 spec에 적고, vendoring한 크레이트(또는 검증한 skarnet.org 타르볼)를 소스 RPM에
넣으므로 COPR은 네트워크 없이 그대로 빌드합니다. 결과는 `DIR/srpms/`와
`DIR/BUILD-ORDER.txt`입니다.

```
stage 1: skalibs-...src.rpm
stage 2: execline-...src.rpm mdevd-...src.rpm
stage 3: s6-...src.rpm
stage 4: s6-rc-...src.rpm s6-linux-init-...src.rpm
stage 5: s6-frontend-...src.rpm
stage 6: sdcompat-...src.rpm sdcompatd-...src.rpm ... sdcompat-notify-...src.rpm
```

skarnet 패키지는 앞 패키지들의 `-devel` 패키지로 빌드하므로, 앞 단계가 COPR
프로젝트에서 성공한 뒤에 다음 단계를 시작하세요(단계는 spec의 `BuildRequires`에서
계산합니다). Rust 패키지는 cargo와 rust만 있으면 되므로 순서가 상관없습니다. 본체 다섯
패키지는 서로의 정확한 버전-릴리스를 요구하므로, 한 번에 만든 다섯 개를 함께 올리세요.

copr.fedorainfracloud.org에서 처음 한 번: New Project를 만들고 Fedora chroot(예:
`fedora-45-x86_64`)를 고릅니다. 외부 저장소는 필요 없습니다. 그다음 빌드마다:

- **웹:** 프로젝트의 Builds > New Build > Upload에서 소스 RPM을 하나씩, 단계 순서대로
  올립니다.
- **copr-cli** (`dnf install copr-cli`, API 토큰은 `~/.config/copr`): 단계마다
  `copr-cli build OWNER/sdcompat FILE...`. 빌드가 끝날 때까지 기다립니다.

skarnet 패키지는 `dist/skarnet.versions`가 바뀔 때만 다시 빌드하면 됩니다. 코드를 고친
뒤에는 `packages/copr-srpms.sh core`의 결과를 올리세요(독립 도구가 바뀌었으면
`standalone`도). 사용자는 다음을 실행합니다.

```sh
sudo dnf copr enable OWNER/sdcompat
sudo dnf install sdcompat
sudo sdcompat init
```

spec에는 아직 `License: TBD`라고 되어 있습니다. COPR은 그대로 빌드하지만, 공개하기 전에
라이선스를 정하세요.

## 직접 빌드한 패키지 설치

```sh
sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm      # skarnet 도구 (dist/rpm/build-skarnet-rpms.sh)
sudo dnf install ~/rpmbuild-sdcompat/rpms/*.rpm
sudo dnf install ~/rpmbuild-sdcompat-standalone/rpms/*.rpm   # 선택 (standalone/*/build-rpm.sh)
sudo sdcompat init
```

Rust 코드를 고친 뒤에는 다시 빌드하고 업그레이드합니다.

```sh
git pull                                            # 또는 직접 고치고 커밋
packages/build-rpms.sh
sudo dnf upgrade ~/rpmbuild-sdcompat/rpms/*.rpm
sdcompat --version                                  # 설치된 빌드 확인
```

s6로 부팅한 상태라면, 새 sdcompatd는 데몬을 다시 시작하면 쓰입니다(`s6-svc -r
/run/service/sdcompatd`). runner와 timers는 서비스가 다음에 시작될 때 쓰이고,
sdcompat-gen은 다음 `sdcompat reload` 때 쓰입니다(`systemctl daemon-reload`는 sdcompatd에
들어 있는 사본을 쓰므로, 데몬이 다시 시작된 뒤에 바뀝니다). 부팅 때는 아무것도 다시 만들지
않습니다.

s6-rc 저장소가 없던 버전(`s6 set` 변경 이전)에서 옮겨 올 때는 `sudo sdcompat init`을 다시
실행하세요. `/etc/sdcompat/s6/default`를 지우고(이제 세트의 커밋이 `default` 번들을 만듭니다.
이것이 있으면 reload가 실행을 거부합니다), 새 `/etc/s6.conf`를 쓰고, `/etc/s6/sources`를
만들고, 새 SELinux 모듈을 불러오고, systemd의 활성화 상태에서 시작하는 세트 `current`와 함께
저장소를 만듭니다.

s6-linux-init 스켈레톤(`rc.init`, `rc.shutdown`, `rc.shutdown.final`)은
다릅니다. 패키지는 init이 쓰는 원본을 설치할 뿐이고, 부팅에 쓰이는 것은
`s6-linux-init-maker`가 그것으로 만든 `/etc/s6-linux-init/current`입니다.
업그레이드로 스켈레톤이 바뀌면 `sudo sdcompat init`을 다시 실행해(지난 실행의 선택을
유지합니다) 새로 만드세요. `early`와 나머지는 그러지 않아도 다음 부팅부터 적용됩니다.

예전 `dist/setup-fedora.sh`로 설치한 시스템에서 옮겨 올 때: 패키지를 설치하고 `sudo
sdcompat init`을 실행하세요. 스크립트가 `/etc/kernel/install.d`에 복사한 kernel-install
플러그인(패키지의 것을 가립니다)을 지우고, 스크립트가 남긴 설정을 읽습니다. 소스로 빌드한
skarnet 도구(`--build-s6`)는 RPM의 파일로 바뀌며, 예전 목록
`/var/lib/sdcompat/skarnet-files.txt`는 지워도 됩니다.

제거: `sudo sdcompat init --uninstall` 다음
`dnf remove sdcompat sdcompat-gen sdcompat-runner sdcompat-timers sdcompatd`.
