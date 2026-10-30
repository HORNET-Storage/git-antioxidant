#!/usr/bin/env bash
set -eu -o pipefail

mkdir a b c d
echo "#!/a/x.exe" > a/x.exe
echo "#!/a/x" > a/x
echo "#!/b/exe" > b/exe
echo "#!/b/exe.com" > b/exe.com
echo "#!/c/x.exe" > c/x.exe
echo "#!/usr/bin/interpreter --option" > d/script
echo "executable placeholder" > d/interpreter.exe
