## Install

macOS 13 or later on Apple Silicon (arm64) only.

1. Download `gridterm-<version>-arm64.zip` and unzip it.
2. Move `gridterm.app` to `/Applications`.
3. The app is signed ad hoc, not with an Apple Developer ID, so macOS blocks it the first time ("cannot be opened" or "is damaged"). To allow it:

   ```sh
   xattr -dr com.apple.quarantine /Applications/gridterm.app
   ```

   or right-click the app → Open → Open.
