# sdcompat-linkwatch

[English](README-us.md) | **한국어**

`s6-rc-update`(daemon-reload)와 `s6-svlink`가 새 감독자(supervisor)의 시작
이벤트를 기다리는데, 그 이벤트가 듣기 전에 와 버려 끝없이 기다리는 경우를 풀어
줍니다.

sdcompat 프로젝트에서 떼어 낸 독립 도구입니다. 설정을 읽지 않으며, 다른 sdcompat
구성 요소 없이 빌드·설치·실행됩니다. 의존성은 libc뿐이고, 지켜보는 대상은 s6입니다.

## 왜 필요한가

두 프로그램이 쓰는 `s6_supervise_link_names()`(s6 2.15)는 먼저 어느 서비스
디렉터리에 이미 감독자가 있는지 확인하고, 나머지의 `event/` fifodir를 구독한 뒤
스캔 디렉터리에 링크하고, 각각의 `s` 이벤트를 기다립니다. 그 사이에 s6-svscan이
감독자를 시작하면(s6-rc-update가 방금 멈춘 옛 감독자를 s6-svscan이 다시 띄우다가
새 링크를 찾는 경우) `s`는 아무도 받지 못합니다. 대기는 끝나지 않습니다.
s6-rc-update는 `-t`를 주지 않으면 시간 제한이 없고, s6-rc의 잠금을 쥐고 있으므로
그 뒤의 모든 s6-rc 명령도 기다리게 됩니다.

## 하는 일

`--interval`초마다 `/proc`에서 부모가 `s6-rc-update`나 `s6-svlink`이고
`--after`초 이상 실행된 `s6-ftrigrd`를 찾습니다(구독은 몇 밀리초면 끝나므로 그런
프로세스는 기다리는 중입니다). 그 s6-ftrigrd가 쥔 `event/ftrig1:*` fifo마다, 서비스
디렉터리의 `supervise/lock`이 잡혀 있으면(감독자가 실행 중이므로 시작 이벤트는 이미
보내졌습니다) 그 fifo에 `s`를 한 번 씁니다. 그 밖에는 손대지 않습니다. 다른
프로그램의 대기(서비스를 기다리는 s6-rc, s6-svwait)는 그대로 두고, 이미 이벤트를
받은 fifo는 중복 이벤트를 받지만 기다리는 쪽은 더 이상 그것을 보지 않습니다.

기다리는 프로세스의 `/proc/PID/fd`를 읽어야 하므로 그 프로세스와 같은 사용자로
실행하세요(실제 시스템에서는 root).

## 빌드

```sh
cargo build --release
cargo test
```

## 설치

```sh
sudo install -Dm0755 target/release/sdcompat-linkwatch /usr/libexec/sdcompat/sdcompat-linkwatch
```

RPM: `./build-rpm.sh [OUTDIR]`가 `sdcompat-linkwatch.spec`을 `~/rpmbuild-sdcompat-standalone/rpms`에
빌드합니다(sudo 불필요. 빌드할 때마다 릴리스가 새로워져 `sudo dnf upgrade`로
올릴 수 있습니다). 설치는 `sudo dnf install ~/rpmbuild-sdcompat-standalone/rpms/sdcompat-linkwatch-*.rpm`이고,
그 뒤로 `setup-fedora.sh`는 이 도구를 rpm에 맡깁니다.
sdcompat은 이것을 s6 서비스 `sdcompat-linkwatch`로 실행합니다(도구가 설치되지
않았으면 아무 일도 하지 않습니다).

## 실행

```sh
sdcompat-linkwatch [--after 초] [--interval 초] [--under 디렉터리] [--once]
```

| 옵션 | 뜻 |
|---|---|
| `--after 초` | 대기가 이만큼 지나면 풉니다(기본 10) |
| `--interval 초` | 얼마나 자주 볼지(기본 2) |
| `--under 디렉터리` | 이 디렉터리 아래의 서비스 디렉터리만(시험용 s6 트리) |
| `--once` | 한 번 보고 끝냅니다 |

보낸 이벤트마다 stderr에 기록합니다:
`sdcompat-linkwatch: s6-rc-update (pid N) waited 12 s for DIR, whose supervisor runs: sent its start event`.
