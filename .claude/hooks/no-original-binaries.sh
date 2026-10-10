#!/bin/sh
# Refuses any tool call that names the original's program files or a
# disassembler (.claude/CLAUDE.md, "Where game rules come from").
input=$(cat)
if printf '%s' "$input" | grep -qiE 'recoil(3dfx)?\.exe|(^|[^[:alnum:]_])(objdump|llvm-objdump|ghidra|analyzeheadless|radare2|rabin2|rizin|ndisasm|x64dbg|winedbg|pefile)([^[:alnum:]_]|$)'; then
  echo "Blocked: this project does not open, inspect or disassemble the original's program files (.claude/CLAUDE.md)." >&2
  exit 2
fi
exit 0
