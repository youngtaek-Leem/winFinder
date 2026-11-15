# winFinder

macOS Finder 스타일의 Windows x64 파일 탐색기입니다. Rust + `eframe`/`egui`로 작성되었습니다.

## 주요 기능

- **빠른 파일 목록**: Windows `FindFirstFileExW` 배치 API(`FindExInfoBasic`, `FIND_FIRST_EX_LARGE_FETCH`) 사용, 가상화 리스트로 수만 개 항목도 부드럽게 표시
- **Finder 스타일 UI**: 사이드바(즐겨찾기/드라이브), 브레드크럼, 정렬(이름/크기/수정일/유형), 숨김 파일 토글, 현재 폴더 필터
- **미리보기**: `Space`로 Quick Look 스타일 미리보기
  - 이미지, 텍스트/코드(CP949/EUC-KR 자동 감지), Markdown, PDF, 오디오, 동영상 재생
  - 대용량 파일은 앞부분만 표시, 미지원 형식은 메타데이터 카드 표시
- **멀티뷰**: 최대 2×2(4패널), `1×1 / 1×2 / 2×2` 레이아웃, 패널별 독립 경로/히스토리/정렬/스크롤
- **탭**: 패널당 최대 10개 탭, 패널별 히스토리 유지
- **세션 복원**: 종료 후 재실행 시 창 크기/위치, 레이아웃, 각 패널의 탭·경로·정렬·스크롤 복원
- **파일 작업**: 새 폴더, 이름 변경, 휴지통으로 삭제
- **한글 지원**: Pretendard 폰트 번들, 한국어 파일명/인코딩 대응

## 단축키

| 키 | 동작 |
|---|---|
| `Space` | 미리보기 열기/닫기 |
| `Esc` | 미리보기 닫기 |
| `↑` / `↓` | 선택 이동 (미리보기 열린 상태에서도 대상 변경) |
| `Enter` | 열기 |
| `Backspace` | 상위 폴더 |
| `Ctrl/Cmd + ←` / `→` | 뒤로 / 앞으로 |
| `Ctrl/Cmd + T` / `W` | 새 탭 / 탭 닫기 (최대 10개) |
| `Ctrl/Cmd + Tab` / `Shift + Tab` | 다음 / 이전 탭 |
| `Ctrl/Cmd + 1..4` | 패널 포커스 이동 |
| `F2` | 이름 변경 |
| `Delete` | 휴지통으로 삭제 |
| `Ctrl/Cmd + Shift + N` | 새 폴더 |

## 개발 환경

- Rust stable (2024 edition)
- macOS에서 개발/실행 가능 (Windows 전용 셸 기능은 `#[cfg(windows)]`로 격리)

```bash
cargo run -p winfinder-app
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Windows x64 빌드

GitHub Actions의 macOS 러너에서 `cargo-xwin`으로 `x86_64-pc-windows-msvc` 대상 크로스 컴파일합니다.

```bash
rustup target add x86_64-pc-windows-msvc
cargo install cargo-xwin --locked
cargo xwin build -p winfinder-app --release --target x86_64-pc-windows-msvc
```

결과물: `target/x86_64-pc-windows-msvc/release/winFinder.exe`

## 릴리스

최신 다운로드: <https://github.com/youngtaek-Leem/winFinder/releases/latest>

`v*` 태그를 push하면 `.github/workflows/build-windows.yml`이 실행되어 다음을 GitHub Release에 첨부합니다.

- `winFinder-<version>-windows-x64.zip` (exe + `pdfium.dll` + 문서 + 폰트 라이선스)
- `SHA256SUMS.txt`

수동 실행(`workflow_dispatch`)은 빌드 아티팩트만 업로드합니다.

## 상태 저장 위치

- Windows: `%APPDATA%\winFinder\state.json`
- macOS(개발): `~/Library/Application Support/winFinder/state.json`

## 미리보기 의존성

- PDF 미리보기는 `pdfium.dll`이 필요합니다. 릴리스 zip에 포함되며, 없으면 메타데이터 카드로 대체됩니다.
- 오디오는 `rodio`로 재생합니다.
- 동영상은 Windows Media Foundation으로 재생합니다. 오디오 트랙은 최대 128MB PCM까지 미리 디코딩하며, 이를 넘는 긴 영상은 무음으로 재생됩니다.

## 실행 문제 해결

- 다운로드한 zip 파일의 속성에서 **차단 해제(Unblock)** 를 선택한 뒤 압축을 완전히 풀어 주세요.
- Windows SmartScreen이 표시되면 **추가 정보 → 실행** 을 선택하세요.
- v0.1.2부터 Visual C++ 재배포 패키지가 필요 없도록 정적 CRT로 빌드합니다.
- 실행 중 오류 창이 표시되면 `%APPDATA%\winFinder\startup-error.log` 또는 `crash.log` 내용을 확인해 주세요.
- v0.1.3부터 Windows에서는 OpenGL 대신 **WGPU(DirectX 12)** 렌더러를 기본으로 사용합니다. DirectX 12 하드웨어 가속이 없으면 Windows의 WARP 소프트웨어 렌더러로 실행됩니다.
- UTM 같은 가상 머신에서 화면이 표시되지 않으면 게스트의 DirectX/GPU 드라이버(UTM의 실험적 DirectX 지원 등)를 확인하세요.
- 렌더러를 강제로 바꾸려면 실행 전에 환경 변수를 지정합니다.
  - PowerShell: `$env:WINFINDER_RENDERER="glow"; .\winFinder.exe` (또는 `"wgpu"`)
  - 명령 프롬프트: `set WINFINDER_RENDERER=glow` 후 `winFinder.exe`
  - macOS 개발 빌드는 기본이 OpenGL(`glow`)이며, `WINFINDER_RENDERER=wgpu`로 WGPU를 사용할 수 있습니다.

## 라이선스

MIT. 번들 폰트 및 PDFium 라이선스는 `THIRD_PARTY_NOTICES.md`를 참고하세요.
