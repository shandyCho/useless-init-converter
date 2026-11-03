# uinitconverter RPM 패키지

[English](README-us.md)

본체(`crates/`)의 실행 파일은 각각 RPM 패키지 하나가 되고, s6 사용자 매니저도 자기 패키지가
있습니다(`uinitconverter-user`, `initconvertd`로 들어가는 진입점). 소스는 Cargo 작업공간
하나에 그대로 있고, 여섯 패키지 모두 같은 소스 타르볼로 빌드합니다. 라이브러리
크레이트(`uinitconverter-base`, `uinitconverter-install`, `uinitconverter-exec`, `uinitconverter-client`)는
실행 파일에 정적으로 링크되므로 빌드할 때만 쓰이고, 따로 패키지가 되지 않습니다.

| 패키지 | 들어 있는 것 | 실행할 때 필요한 패키지 (`Requires`) |
|---|---|---|
| `uinitconverter-runner` | `initconvert-runner` | s6, shadow-utils. `uinitconverterd`는 권장(`OnFailure=`/`OnSuccess=` 유닛을 데몬으로 시작) |
| `uinitconverterd` | `initconvertd`, D-Bus 정책(`uinitconverter.conf`) | s6, `uinitconverter-runner`. `uinitconverter-timers`는 권장(`systemd-run --on-calendar`), `uinitconverter-user`는 제안 |
| `uinitconverter-user` (noarch) | `initconvert-user-manager`(`initconvertd`로의 링크. 이 이름으로 실행되면 사용자 매니저로 동작), `initconvert-user-setup`(systemd 부팅에서 켜기/끄기/상태), `/run/uinitconverter-user`(대체 기록)용 tmpfiles.d 항목 | `uinitconverterd`, `uinitconverter-runner`, `uinitconverter-timers`, s6, systemd(`systemd --user`가 기본값이자 대체). s6 부팅 패키지는 필요 없습니다. 이 패키지가 없으면 `USER_MANAGER=`와 상관없이 `user@`는 `systemd --user`를 씁니다 |
| `uinitconverter-timers` | `initconvert-timers` | `uinitconverterd` (타이머가 유닛을 데몬으로 시작) |
| `uinitconverter-gen` | `initconvert-gen`, `/var/lib/uinitconverter` | s6-rc, `uinitconverter-runner`, `uinitconverter-timers` (생성된 서비스가 실행) |
| `uinitconverter` | `initconvert` 명령(부팅 설정 `initconvert init` 포함), `early`, 설정, s6 서비스, s6-linux-init 골격, SELinux 모듈, kernel-install 플러그인, `/var/log/uinitconverter` | s6, s6-linux-init, s6-rc, execline, dbus-daemon, util-linux, policycoreutils-python-utils, grubby, 그리고 위의 도구 패키지 네 개. `uinitconverter-user`는 제안 |

- `dnf install uinitconverter`로 전부 설치됩니다. 부팅에 필요한 설정과 SELinux 모듈은
  `uinitconverter` 패키지에 있으므로, s6로 부팅하려면 이 패키지가 있어야 합니다.
- 도구 패키지는 따로 업데이트할 수 있습니다. 같은 버전끼리만 맞도록
  (`= 버전-릴리스`) 묶여 있습니다.
- 독립 도구(`standalone/`)는 여기에 없습니다. 각 디렉터리에 자체 spec과
  `build-rpm.sh`(결과: `~/rpmbuild-uinitconverter-standalone/rpms`)가 있고,
  `uinitconverter`는 이들을 `Recommends`/`Suggests`로만 가리킵니다.

## 빌드

테스트 빌드와 릴리스 빌드를 나란히 놓은 단계별 설명은 [docs/PACKAGEBUILD.md](../docs/PACKAGEBUILD.md)에
있습니다.

```sh
packages/build-rpms.sh            # 결과: ~/rpmbuild-uinitconverter/rpms
packages/build-rpms.sh DIR        # 다른 위치
packages/build-rpms.sh --srpm     # 소스 RPM만: ~/rpmbuild-uinitconverter/srpms
packages/build-rpms.sh --test     # 테스트용 빠른 빌드(아래 참고)
```

스크립트는 작업공간(`Cargo.toml`, `Cargo.lock`, `crates`, `dist`)과 `cargo vendor`로
받은 크레이트를 `uinitconverter-VERSION.tar.gz` 하나로 묶고, spec마다 `rpmbuild -bb`를
실행합니다. rpmbuild는 네트워크 없이(`--offline`) 빌드합니다. 아무것도 설치하지 않고
sudo도 쓰지 않습니다. 필요한 것: `rpm-build`, `cargo`, `rust`.

**테스트용 빠른 빌드.** spec마다 자기 트리에서 빌드하므로, 보통 빌드는 모든 크레이트를
여섯 번 컴파일하고 spec마다 `%check`에서 `cargo test`를 실행합니다. VM 등에서 시험만 할
RPM이라면 다음 옵션으로 줄일 수 있습니다(기본은 꺼짐):

| 옵션 | 효과 |
|---|---|
| `--shared-target` | 모든 spec이 고정된 경로의 소스 사본 하나에서 cargo 타깃 디렉터리 하나로 빌드합니다(둘 다 `OUTDIR/cargo-target` 아래). 그래서 크레이트를 spec마다가 아니라 한 번만 컴파일합니다. 남겨 두므로 다음 빌드는 바뀐 것만 다시 컴파일합니다. debuginfo·debugsource 패키지는 만들지 않습니다 |
| `--nocheck` | rpmbuild가 `%check`를 건너뜁니다(`cargo test` 없음. 트리에서 직접 실행하세요) |
| `--test` | 둘 다 |

배포할 RPM에는 쓰지 마세요. 배포용은 `github-release.sh`처럼 두 옵션 없이 빌드합니다.
패키지 내용은 같지만, spec마다의 빌드가 서로 분리되지 않고 테스트도 실행되지 않습니다.

## 버전

spec에는 버전을 적지 않습니다. 스크립트가 정해서 전달하므로 spec은 이 스크립트로
빌드하세요(`rpmbuild`를 직접 실행하면 버전이 없어 실패합니다).

- **버전**은 `Cargo.toml`의 `[workspace.package] version` 하나에서 읽습니다. 크레이트들도
  이 값을 쓰므로, 버전을 올릴 때는 이 줄만 수정하면 됩니다.
- **릴리스**는 빌드할 때마다 새로 정합니다: `1.YYYYMMDDHHMMSS.g커밋`. 커밋하지 않은
  변경이 있으면 끝에 `.dirty`가 붙습니다. 예: `uinitconverter-gen-0.1.0-1.20261021071849.g0023111.fc45`.
  빌드 시각이 앞에 오므로, 버전을 그대로 두고 Rust 코드만 수정해서 다시 빌드해도 dnf는
  새 빌드를 업그레이드로 봅니다.
- 여섯 패키지는 같은 버전-릴리스끼리만 맞도록 묶여 있고, 스크립트는 언제나 여섯 개를
  함께 빌드합니다. `rpms/`에는 마지막 빌드의 여섯 개만 남습니다.
- RPM으로 빌드한 도구의 `--version`은 패키지의 버전-릴리스를 보여 줍니다(예:
  `initconvert-gen 0.1.0-1.20261021073517.gc8aabf8.fc45`). 소스에서 빌드하면
  `0.1.0 (git c8aabf8)`처럼 커밋을 보여 줍니다.
- 공개 릴리스처럼 릴리스 번호를 직접 정하려면 `UINITCONVERTER_RELEASE=1 packages/build-rpms.sh`.

## 변경 이력과 릴리스 노트

릴리스 전에 `CHANGELOG.md`/`CHANGELOG-us.md`에 해당 버전 항목을 추가하고
`packages/release-notes/VERSION.md`/`VERSION-us.md`를 작성합니다.
`github-release.sh`가 워크스페이스 버전의 노트를 `NOTES.md`에 넣습니다.

## GitHub 릴리스

패키지는 프로젝트의 [GitHub 릴리스](https://github.com/shandyCho/useless-init-converter/releases)에
파일로 업로드합니다.

```sh
packages/github-release.sh             # 결과: ~/uinitconverter-release
packages/github-release.sh DIR         # 다른 위치
```

커밋된 트리가 필요하고, 새로 만든 `DIR/work`에서 `packages/build-rpms.sh`와
`standalone/*/build-rpm.sh`를 실행한 뒤 다음을 모읍니다.

- `DIR/assets/`: 바이너리 RPM(본체 여섯 패키지와 독립 도구. debuginfo 패키지는 빼고)과
  `SHA256SUMS`. skarnet.org 패키지는 릴리스에 넣지 않습니다. 사용자가 각자
  `dist/rpm/build-skarnet-rpms.sh`로 빌드합니다.
- `DIR/NOTES.md`: 릴리스 본문. 커밋, RPM을 빌드한 Fedora 릴리스와 아키텍처, 설치 명령,
  패키지 목록이 들어 있습니다.

끝에 GitHub CLI가 로그인된 곳에서 실행할 `gh release create` 명령을 출력합니다(태그는
`[workspace.package] version`에서 만든 `vVERSION`). CLI 없이 하려면 GitHub에서
Releases > Draft a new release를 열고, 그 커밋에 태그 `vVERSION`을 지정하고, NOTES.md를
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
initconvert init --dry-run
sudo initconvert init
```

rsyslog, earlyoom 같은 다른 의존 패키지는 dnf가 Fedora 저장소에서 가져옵니다. GitHub에는
저장소가 없으므로 `dnf upgrade`만으로는 새 릴리스를 알 수 없습니다. 다음 릴리스의 파일을
받아 `sudo dnf upgrade ./*.rpm`을 실행한 뒤 `sudo initconvert init --yes`를 실행하세요. skarnet
패키지는 `dist/skarnet.versions`가 바뀔 때만 다시 빌드하면 됩니다.

spec에는 아직 `License: TBD`라고 되어 있습니다. 공개하기 전에 라이선스를 정하세요.

## 직접 빌드한 패키지 설치

```sh
sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm      # skarnet 도구 (dist/rpm/build-skarnet-rpms.sh)
sudo dnf install ~/rpmbuild-uinitconverter/rpms/*.rpm
sudo dnf install ~/rpmbuild-uinitconverter-standalone/rpms/*.rpm   # 선택 (standalone/*/build-rpm.sh)
sudo initconvert init
```

Rust 코드를 수정한 뒤에는 다시 빌드하고 업그레이드합니다.

```sh
git pull                                            # 또는 직접 수정하고 커밋
packages/build-rpms.sh
sudo dnf upgrade ~/rpmbuild-uinitconverter/rpms/*.rpm
initconvert --version                                  # 설치된 빌드 확인
```

s6로 부팅한 상태라면, 새 initconvertd는 데몬을 다시 시작하면 적용됩니다(`s6-svc -r
/run/service/uinitconverterd`). runner와 timers는 서비스가 다음에 시작될 때 적용되고,
initconvert-gen은 다음 `initconvert reload` 때 적용됩니다(`systemctl daemon-reload`는 initconvertd에
들어 있는 사본을 쓰므로, 데몬이 다시 시작된 뒤에 바뀝니다). 부팅 때는 아무것도 다시 만들지
않습니다.

s6-rc 저장소가 없던 버전(`s6 set` 변경 이전)에서 이전할 때는 `sudo initconvert init`을 다시
실행하세요. `/etc/uinitconverter/s6/default`를 지우고(이제 세트의 커밋이 `default` 번들을 만듭니다.
이것이 있으면 reload가 실행을 거부합니다), 새 `/etc/s6.conf`를 쓰고, `/etc/s6/sources`를
만들고, 새 SELinux 모듈을 불러오고, systemd의 활성화 상태에서 시작하는 세트 `current`와 함께
저장소를 만듭니다.

s6-linux-init 골격(`rc.init`, `rc.shutdown`, `rc.shutdown.final`)은
다릅니다. 패키지는 init이 쓰는 원본을 설치할 뿐이고, 부팅에 쓰이는 것은
`s6-linux-init-maker`가 그것으로 만든 `/etc/s6-linux-init/current`입니다.
업그레이드로 골격이 바뀌면 `sudo initconvert init`을 다시 실행해(지난 실행의 선택을
유지합니다) 새로 만드세요. `early`와 나머지는 그러지 않아도 다음 부팅부터 적용됩니다.

### initconvert 명령으로 이전하기

명령의 이름이 `uinitconverter*`에서 `initconvert*`로 바뀌었습니다. `initconvert`(`/usr/bin/initconvert`),
`/usr/libexec/uinitconverter/`의 `initconvertd`, `initconvert-gen`, `initconvert-runner`,
`initconvert-timers`, `initconvert-user-manager`, 그리고 독립 도구 `initconvert-fdstore`,
`initconvert-notify`, `initconvert-journal-relay`, `initconvert-linkwatch`입니다. 나머지는 이름이 그대로입니다.
패키지, `/etc/uinitconverter`, `/var/lib/uinitconverter`, `/run/uinitconverter`, s6 서비스
(`uinitconverterd`, `uinitconverter-fdstore` 등), SELinux 모듈, D-Bus 인터페이스, 커널 옵션,
`UINITCONVERTER_*` 변수. 자동 이전은 없습니다. s6-rc 데이터베이스와 s6-linux-init 트리는 프로그램을
경로로 가리키므로, 업그레이드한 뒤 다시 만들어야 합니다. **일반 systemd 부팅**에서:

```sh
sudo dnf upgrade ~/rpmbuild-uinitconverter/rpms/*.rpm \
    ~/rpmbuild-uinitconverter-standalone/rpms/*.rpm   # 또는 릴리스의 RPM
sudo initconvert init          # 지난 실행과 같은 선택으로: 데이터베이스를 다시 만들고
                               # s6-linux-init 트리를 다시 빌드합니다
```

`/etc/uinitconverter/s6` 아래의 서비스를 고친 적이 있다면(`%config(noreplace)`입니다), dnf는 옛 경로를
담은 그 사본을 두고 새 파일을 `.rpmnew`로 옆에 놓습니다. `initconvert init`을 실행하기 전에 새 경로를
옮겨 적으세요(`sudo find /etc/uinitconverter -name '*.rpmnew'`).

그다음 s6 항목으로 부팅하세요. `uinitconverter`, `uinitconverterd` 등 옛 이름을 부르는 직접 만든
스크립트는 새 이름으로 고쳐야 합니다. 설정과 s6-rc 세트는 그대로 유지됩니다. s6로 부팅한 상태에서
업그레이드하지 마세요. `initconvert init`을 실행하기 전까지는 (다시) 시작하는 서비스가 사라진 옛 경로를
실행합니다.

### sdcompat에서 이전하기

이 프로젝트의 예전 이름은 sdcompat입니다. 모든 이름이 바뀌었습니다(명령, 패키지,
`/etc/sdcompat` → `/etc/uinitconverter`, `/var/lib/sdcompat`, SELinux 모듈, D-Bus
인터페이스, `sdcompat.db=` → `uinitconverter.db=`, GRUB 항목). 자동 이전은 없습니다.
sdcompat을 제거한 뒤 uinitconverter를 새로 설치하세요. **일반 systemd 부팅**에서:

```sh
sudo cp -a /etc/sdcompat ~/sdcompat-etc     # 설정. 나중에 직접 옮깁니다
cat /etc/sdcompat/boot-options              # 지난 init 실행의 선택
sudo sdcompat init --uninstall              # s6 부팅 항목, initramfs, s6-linux-init 트리
sudo dnf remove sdcompat sdcompat-gen sdcompat-runner sdcompat-timers sdcompatd \
    sdcompat-fdstore sdcompat-journal-relay sdcompat-linkwatch sdcompat-notify
sudo mv /var/lib/s6-rc/repository /var/lib/s6-rc/repository.sdcompat
sudo rm -rf /var/lib/sdcompat /var/log/sdcompat
```

패키지를 제거하면 예전 SELinux 모듈도 제거됩니다. s6-rc 저장소는 세트가 아직 예전 서비스
이름을 담고 있으므로 다른 이름으로 옮겨 둡니다. `initconvert init`이 systemd의 활성화 상태에서 새
저장소를 만듭니다(직접 적용한 `s6 set` 변경은 다시 적용해야 합니다). 그다음 uinitconverter RPM을
설치하고 예전에 쓰던 옵션으로 `sudo initconvert init`을 실행하세요(예: `--journal=no
--boot-only-s6=yes --selinux=enforcing`. 예전 선택은 읽지 않습니다). 수정한 설정은
`~/sdcompat-etc`에서 `/etc/uinitconverter`로 직접 옮기세요(파일 이름은 같고
`sdcompat.conf`만 `uinitconverter.conf`가 됩니다).

제거: `sudo initconvert init --uninstall` 다음
`dnf remove uinitconverter uinitconverter-gen uinitconverter-runner uinitconverter-timers uinitconverterd uinitconverter-user`.
