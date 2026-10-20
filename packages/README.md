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
| `sdcompat` | `sdcompat` 명령, `early`, 설정, s6 서비스, s6-linux-init 골격, SELinux 모듈, kernel-install 플러그인, `/var/log/sdcompat` | s6, s6-linux-init, s6-rc, execline, dbus-daemon, util-linux, 그리고 위의 네 패키지 |

- `dnf install sdcompat`이면 전부 설치됩니다. 부팅에 필요한 설정과 SELinux 모듈은
  `sdcompat` 패키지에 있으므로, s6로 부팅하려면 이 패키지가 있어야 합니다.
- 도구 패키지는 따로 업데이트할 수 있습니다. 같은 버전끼리만 맞도록
  (`= 버전-릴리스`) 묶여 있습니다.
- 독립 도구(`standalone/`)는 여기에 없습니다. 각 디렉터리에 자기 spec이 있고,
  `sdcompat`은 이들을 `Recommends`/`Suggests`로만 가리킵니다.

## 빌드

```sh
packages/build-rpms.sh            # 결과: ~/rpmbuild-sdcompat/rpms
packages/build-rpms.sh DIR        # 다른 위치
```

스크립트는 작업공간(`Cargo.toml`, `Cargo.lock`, `crates`, `dist`)과 `cargo vendor`로
받은 크레이트를 `sdcompat-VERSION.tar.gz` 하나로 묶고, spec마다 `rpmbuild -bb`를
실행합니다. rpmbuild는 네트워크 없이(`--offline`) 빌드합니다. 아무것도 설치하지 않고
sudo도 쓰지 않습니다. 필요한 것: `rpm-build`, `cargo`, `rust`.

spec 다섯 개의 `Version:`은 같아야 합니다(타르볼 하나를 함께 씀). 다르면 스크립트가
멈춥니다.

## 설치

```sh
sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm      # skarnet 도구 (dist/rpm/build-skarnet-rpms.sh)
sudo dnf install ~/rpmbuild-sdcompat/rpms/*.rpm
sudo dist/setup-fedora.sh --rpms=$HOME/rpmbuild-skarnet/rpms
```

`setup-fedora.sh`는 `sdcompat` RPM이 설치되어 있으면 본체를 빌드하지 않고 그것을
씁니다. 도구 RPM 일부만 설치되어 있으면 그 도구는 건드리지 않고 나머지만 소스에서
설치합니다. 제거는 `dnf remove sdcompat sdcompat-gen sdcompat-runner sdcompat-timers sdcompatd`입니다.
