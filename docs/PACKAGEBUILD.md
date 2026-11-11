# 패키지 빌드: 테스트 빌드와 릴리스 빌드

[English](PACKAGEBUILD-us.md) | **한국어**

테스트용과 릴리스용 RPM을 만드는 단계와 명령입니다. 각 패키지에 무엇이 들어 있는지는
[packages/README.md](../packages/README.md)에 있습니다.

## 개요

패키지는 세 묶음이고, 묶음마다 빌드 스크립트가 있습니다:

| 묶음 | 패키지 | 스크립트 | 결과(기본 위치) |
|---|---|---|---|
| skarnet.org | skalibs, execline, s6, s6-rc, s6-linux-init, s6-frontend, mdevd, s6-dns, s6-networking | `dist/rpm/build-skarnet-rpms.sh` | `~/rpmbuild-skarnet/rpms` |
| 본체 | uinitconverter, uinitconverterd, uinitconverter-gen, -runner, -timers | `packages/build-rpms.sh` | `~/rpmbuild-uinitconverter/rpms` |
| 독립 도구 | uinitconverter-fdstore, -notify, -linkwatch, -journal-relay | `standalone/*/build-rpm.sh` | `~/rpmbuild-uinitconverter-standalone/rpms` |

어느 스크립트도 아무것도 설치하지 않고 sudo도 쓰지 않습니다. Rust 크레이트를 소스 tarball에
vendor로 넣으므로 rpmbuild는 네트워크 없이 빌드합니다.

**테스트 빌드와 릴리스 빌드**

| | 테스트 빌드 | 릴리스 빌드 |
|---|---|---|
| 용도 | 변경 시험: VM, 자기 컴퓨터 | GitHub에 배포 |
| 본체 명령 | `packages/build-rpms.sh --test DIR` | `packages/github-release.sh` |
| 작업 트리 | 커밋하지 않은 변경이 있어도 됨(릴리스 번호에 `.dirty`) | 커밋되어 있어야 함 |
| 시간(본체 패키지) | 3-4분쯤, 캐시를 남겨 두면 더 짧음 | 11분쯤 |
| rpmbuild 안의 `cargo test`(`%check`) | 건너뜀 | 실행 |
| debuginfo/debugsource 패키지 | 없음 | 만듦(배포하지 않음) |
| 버전 올리기, 변경 이력, 노트 | 아니요 | 예 |

어느 쪽이든 패키지에 들어가는 파일은 같습니다. 테스트 빌드는 rpmbuild 안의 테스트를 건너뛰고,
spec들이 컴파일을 한 번만 나눠 쓸 뿐입니다.

## 0. 한 번만: 빌드 도구와 skarnet.org 패키지

```sh
sudo dnf install -y git curl rpm-build gcc make cpio cargo rust
```

skarnet.org 패키지는 uinitconverter를 설치하고 실행할 때 필요하고, 빌드할 때는 필요 없습니다.
Fedora에는 이 패키지가 없습니다. `dist/skarnet.versions`에 고정된 버전을 받은 뒤(스크립트는 파일마다
SHA-256을 확인하고 직접 받지는 않습니다) 빌드합니다:

```sh
mkdir -p ~/skarnet-sources
grep -v '^#' dist/skarnet.versions | while read -r name ver _; do
    [ -n "$name" ] || continue
    curl -fL -o ~/skarnet-sources/"$name-$ver.tar.gz" \
        "https://skarnet.org/software/$name/$name-$ver.tar.gz"
done
dist/rpm/build-skarnet-rpms.sh          # 결과: ~/rpmbuild-skarnet/rpms
```

`dist/skarnet.versions`가 바뀔 때만 다시 빌드합니다. 옵션: `--sources DIR`(tarball이 다른 곳에 있을 때),
`--srpm`(소스 RPM만), 마지막 인자로 결과 디렉터리. mock이 설치되어 있고 `mock` 그룹에 속해 있으면
깨끗한 chroot에서 빌드합니다.

## 1. 테스트 빌드

### 빌드

```sh
packages/build-rpms.sh --test ~/uinitconverter-test          # 본체: ~/uinitconverter-test/rpms
for s in standalone/*/build-rpm.sh; do "$s"; done             # 독립 도구(바뀌었을 때만)
```

`--test`는 옵션 두 개를 합친 것이고, 하나씩 쓸 수도 있습니다:

| 옵션 | 하는 일 |
|---|---|
| `--shared-target` | 모든 spec이 고정된 경로의 소스 사본 하나에서 cargo 타깃 디렉터리 하나로 빌드합니다(둘 다 `DIR/cargo-target` 아래). 그래서 cargo가 크레이트를 spec마다가 아니라 한 번만 컴파일합니다. 이 디렉터리를 남겨 두므로, 같은 `DIR`에서 하는 다음 빌드는 바뀐 것만 다시 컴파일합니다. debuginfo·debugsource 패키지는 만들지 않습니다 |
| `--nocheck` | rpmbuild가 `%check`를 건너뛰므로 그 안에서 `cargo test`가 실행되지 않습니다. 테스트는 작업 트리에서 직접 실행하세요(아래) |

테스트 빌드에는 같은 `DIR`을 계속 쓰세요. 남겨 둔 캐시 덕분에 다음 빌드가 빨라집니다. 개발용
컴퓨터에서 본체를 빌드한 시간(당시 -user까지 여섯 개): 옵션 없이 11분쯤, `--test`로 캐시가 비어 있을 때 253초,
캐시를 남겨 두었을 때 198초. 나머지 시간은 대부분 바이너리마다 하는 최적화(LTO) 링크입니다.

빌드할 때마다 릴리스 번호가 새로 정해집니다: `1.YYYYMMDDHHMMSS.g커밋`. 커밋하지 않은 변경이 있으면
`.dirty`가 붙습니다. 그래서 버전이 같아도 dnf는 다시 빌드한 패키지를 언제나 업그레이드로 봅니다.

### 테스트 실행

`--nocheck`가 건너뛴 테스트와 샌드박스 테스트입니다(root가 필요 없고 호스트는 바뀌지 않습니다):

```sh
cargo test
for p in standalone/*/; do cargo test --manifest-path "$p/Cargo.toml"; done
tests/sandbox.sh /tmp/sb && tests/check.sh /tmp/sb; tests/sandbox.sh /tmp/sb stop
```

변형(`SANDBOX_JOURNAL=no`, `SANDBOX_FDSTORE=no`, `SANDBOX_S6NET_BIN=`)은 [TESTING.md](../TESTING.md)를
참고하세요.

### VM에서 RPM 시험

GNOME 테스트 VM(`tests/gnome-vm.sh fetch`와 `provision`으로 한 번 준비, TESTING.md 참고)은 실행할 때마다
새 사본에 RPM을 설치합니다:

```sh
# s6 부팅: RPM, initconvert init, 재부팅, 확인.
CORE_RPMS=~/uinitconverter-test/rpms tests/vm-s6-boot.sh ~/uinitconverter-vm
# s6 부팅에서 systemd --user 아래의 GNOME 세션.
BOOT=s6 CORE_RPMS=~/uinitconverter-test/rpms tests/vm-user-manager.sh ~/uinitconverter-vm session
```

`vm-s6-boot.sh`는 독립 도구 RPM을 `~/uinitconverter-vm/rpmbuild/standalone/rpms`(`STANDALONE_RPMS=`로
바꿈)에서, skarnet.org RPM을 `~/rpmbuild-skarnet/rpms`(`SKARNET_RPMS=`)에서 가져옵니다.

### 자기 컴퓨터에 설치

VM이면 스냅샷을 만들고, 아니면 돌아갈 일반 systemd 부팅이 있는지 확인하세요. 처음에는:

```sh
sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm
sudo dnf install ~/uinitconverter-test/rpms/*.rpm ~/rpmbuild-uinitconverter-standalone/rpms/*.rpm
sudo initconvert init
```

다시 빌드한 뒤에는 systemd 부팅에서:

```sh
sudo dnf upgrade ~/uinitconverter-test/rpms/*.rpm
sudo initconvert init --yes
```

`rpms/`에는 마지막 빌드의 패키지만 남으므로 `*.rpm`은 언제나 서로 맞는 한 벌입니다(본체 여섯 개는 서로
정확히 같은 버전-릴리스를 요구합니다).

## 2. 릴리스 빌드

릴리스는 본체와 독립 도구 RPM, `SHA256SUMS`, 릴리스 글을 프로젝트의 GitHub 릴리스에 올립니다.
skarnet.org 패키지는 들어가지 않습니다. 사용자가 0단계처럼 직접 빌드합니다.

### 2.1 릴리스 커밋 준비

1. **버전.** `Cargo.toml`의 `[workspace.package] version`을 바꾸고 `cargo build`를 실행해
   `Cargo.lock`도 따라가게 합니다. 태그는 한 번만 쓸 수 있으므로 릴리스마다 새 버전이 필요합니다.
   독립 도구는 버전이 따로 있습니다(자기 `Cargo.toml`과 spec). 바뀌었을 때만 올리세요.
2. **README 쌍.** `README.md`와 `README-us.md` 설치 단계의 `VERSION=`.
3. **변경 이력 쌍.** `CHANGELOG.md`와 `CHANGELOG-us.md`에서 `미출시`/`Unreleased` 항목의 이름을 버전과
   날짜로 바꿉니다.
4. **릴리스 노트 쌍.** `packages/release-notes/VERSION.md`와 `VERSION-us.md`를 씁니다(형식은 이전
   노트를 복사). `github-release.sh`가 이것을 릴리스 글에 넣습니다.
5. **spec 변경 이력.** `packages/`의 spec 여섯 개 각각의 `%changelog`에 항목을 추가합니다. spec에는
   버전과 릴리스를 적지 않습니다. `build-rpms.sh`가 정합니다.

### 2.2 릴리스 트리 테스트

태그를 붙일 바로 그 트리에서 전체 테스트를 실행합니다:

```sh
cargo fmt --check && cargo test
for p in standalone/*/; do cargo test --manifest-path "$p/Cargo.toml"; done
for v in "" SANDBOX_JOURNAL=no SANDBOX_FDSTORE=no; do
    env $v tests/sandbox.sh /tmp/sb && env $v tests/check.sh /tmp/sb; env $v tests/sandbox.sh /tmp/sb stop
done
```

그다음 이 트리로 만든 RPM으로 1절의 VM 실행을 하고, 부팅에 관련된 변경이면 실제 부팅도 한 번 이상
해 봅니다.

### 2.3 커밋과 빌드

```sh
git add -A && git commit -m "Release VERSION"
packages/github-release.sh                 # 결과: ~/uinitconverter-release
```

`github-release.sh`는 커밋하지 않은 변경이 있으면 실행하지 않습니다. 새로 만든
`~/uinitconverter-release/work`에서 본체와 독립 도구를 빌드하고(보통 빌드: spec마다 따로, `%check`
실행) 다음을 모읍니다:

- `~/uinitconverter-release/assets/`: 바이너리 RPM(-devel, debuginfo 패키지 제외)과 `SHA256SUMS`
- `~/uinitconverter-release/NOTES.md`: 릴리스 글. 커밋, Fedora 릴리스와 아키텍처, 설치 명령, 패키지
  목록이 들어 있습니다.

결과물 확인:

```sh
cd ~/uinitconverter-release/assets && sha256sum -c SHA256SUMS && ls
```

RPM은 빌드한 컴퓨터의 Fedora 릴리스와 아키텍처용입니다(예: `fc45`, `x86_64`). 지원할 Fedora
릴리스마다 그 릴리스에서 빌드하세요.

명령 앞에 `UINITCONVERTER_RELEASE=N`을 붙이면 빌드 시각과 커밋 대신 릴리스 번호를 직접 정합니다.

### 2.4 배포

GitHub 인증이 필요하므로 로그인되어 있는 곳에서 실행합니다(이 개발용 컴퓨터에는 인증 정보가
없습니다):

```sh
git push origin main
gh release create vVERSION -R shandyCho/useless-init-converter --target COMMIT \
    --title "uinitconverter VERSION" --notes-file ~/uinitconverter-release/NOTES.md \
    ~/uinitconverter-release/assets/*
```

`github-release.sh`가 버전과 커밋을 채운 이 줄을 출력합니다. GitHub CLI가 없으면 GitHub에서 Releases >
Draft a new release를 열고, 그 커밋에 태그 `vVERSION`을 붙이고, 글에 NOTES.md를 넣고, `assets/`의 파일을
모두 첨부합니다.

## 소스 RPM

다른 곳(mock, 빌드 서비스)에서 빌드하려면 스크립트마다 소스 RPM을 만들 수 있습니다. 버전, 릴리스,
vendor tarball이 들어 있어 그대로 네트워크 없이 다시 빌드됩니다:

```sh
dist/rpm/build-skarnet-rpms.sh --srpm          # ~/rpmbuild-skarnet/srpms. skarnet.versions 순서대로 빌드
packages/build-rpms.sh --srpm                  # ~/rpmbuild-uinitconverter/srpms. 여섯 개를 함께
standalone/uinitconverter-fdstore/build-rpm.sh --srpm
```

`--test`, `--shared-target`, `--nocheck`는 바이너리 RPM을 만드는 옵션이라 `--srpm`과 함께 쓸 수 없습니다.

## 문제가 생기면

| 메시지 | 원인과 할 일 |
|---|---|
| `error: P failed, see DIR/P.log` | 패키지 P의 rpmbuild가 실패했습니다. 로그에 rpmbuild의 출력이 있습니다 |
| `uncommitted changes in ...: commit them first` | `github-release.sh`는 커밋된 트리만 빌드합니다 |
| skarnet tarball이 없다고 나오거나 SHA-256이 맞지 않음 | `~/skarnet-sources`에 다시 받으세요. 값은 `dist/skarnet.versions`에 있습니다 |
| `rpmbuild missing` / `cargo missing` | 0단계의 `dnf install` |
| `dnf upgrade`가 아무것도 설치하지 않음 | 릴리스 번호가 그대로입니다. 다시 빌드하거나 `dnf reinstall`을 쓰세요 |
