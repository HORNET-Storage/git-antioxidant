#!/usr/bin/env bash
set -eu -o pipefail

# Files whose `$Id$` keywords git expanded when it checked them out.
git init -q
echo '* ident' > .gitattributes
printf '%s' '$Id$' > single
printf '%s' '$Id$ $Id$ foo' > multiple
printf 'hi\n$Id$\nho\n\t$Id$$Id$$Id$' > lines
git add .gitattributes single multiple lines
git commit -q -m c1
rm single multiple lines
git checkout -- single multiple lines
