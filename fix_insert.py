import re
src = open(r'src\db\writer.rs', encoding='utf-8').read()
m = re.search(r'INSERT INTO ventas \(([^)]+)\) VALUES \(([^)]+)\)', src)
cols = [c.strip() for c in m.group(1).split(',')]
n = len(cols)
qs = m.group(2).count('?')
print(f'columns={n}, placeholders={qs}, diff={qs-n}')
new_vals = '(' + ','.join(['?']*n) + ')'
old = m.group(0)
new = re.sub(r'VALUES \([^)]+\)', f'VALUES {new_vals}', old)
assert new.count('?') == n
fixed = src.replace(old, new)
open(r'src\db\writer.rs', 'w', encoding='utf-8').write(fixed)
print('Fixed: now', new.count('?'), '? for', n, 'columns')
