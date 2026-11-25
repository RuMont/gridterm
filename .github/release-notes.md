## Instalación

Solo macOS 13 o posterior en Apple Silicon (arm64).

1. Descarga `gridterm-<versión>-arm64.zip` y descomprímelo.
2. Mueve `gridterm.app` a `/Applications`.
3. La app está firmada ad-hoc, no con un certificado de Apple, así que macOS la bloquea la primera vez («no se puede abrir» o «está dañada»). Para permitirla:

   ```sh
   xattr -dr com.apple.quarantine /Applications/gridterm.app
   ```

   o haz clic derecho sobre la app → Abrir → Abrir.
