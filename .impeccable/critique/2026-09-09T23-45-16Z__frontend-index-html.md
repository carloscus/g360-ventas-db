---
target: frontend/index.html (CIPSA DB ventas)
total_score: 20
max_score: 40
na_heuristics: 
p0_count: 0
p1_count: 3
timestamp: 2026-09-09T23-45-16Z
slug: frontend-index-html
---
# Critique: frontend/index.html (CIPSA DB ventas)

Veredicto de especificidad: AUTORADO PARA EL PIPELINE, no para el operador. El lenguaje visual (KPIs verdes, chips de dias faltantes, dropzone XLS, formato es-PE) es propio de esta herramienta; nadie mas la usaria sin cambios. Pero la IA refleja la ARQUITECTURA del sistema (raw-first, allowlist, NC/ND, F01, service role, lock huerfano), no las 3 tareas diarias del usuario. Especifico si, pero especifico del desarrollador.

## Nielsen (20/40 - Acceptable)
1. Visibilidad estado: 3 - pill+%/ETA/crono/task-flow/Notification, pero 6 superficies de estado que pueden contradecirse
2. Lenguaje real: 2 - jargon de pipeline en todas partes
3. Control y libertad: 2 - hay detener y confirms, no hay undo, Esc no cierra modales
4. Consistencia: 2 - mezcla alert/confirm/prompt/status/Notification; botones heterogeneos
5. Prevencion errores: 3 - rangoValido, estimacion, confirms; dbclr solo con confirm nativo
6. Reconocimiento vs recall: 2 - tooltips en menu, pero mes se escribe a mano (prompt YYYY-MM)
7. Flexibilidad: 2 - drag-drop y fechas persistidas; cero atajos de teclado
8. Estetica minimalista: 2 - denso, tipografia 9-11px, footer sin valor, info de service role para devs
9. Recuperacion errores: 1 - "error: "+e crudo, truncado a 180 chars, \n invisibles en status de una linea
10. Ayuda: 1 - solo title attrs ocultos

## Cognitive load: 4/8 fallos (ALTO)
1. Single focus: FALLA - 4 KPIs + 2 info + 2 status Supabase + task-flow + pill + alert compiten
2. Chunking: OK
3. Grouping: OK
4. Jerarquia visual: FALLA - todo a 10-16px; 4 KPIs verdes del mismo peso
5. One thing at a time: FALLA - 6 superficies de estado simultaneas posibles
6. Minimal choices: FALLA - #moreMenu 11 items; config 10+ campos y 4 acciones
7. Working memory: FALLA - mensajes que remiten a controles ("use Limpiar cache o detener"); import exige mes elegido + archivo
8. Progressive disclosure: OK - menu de reparacion oculto bajo "..." es lo mejor

## Emotional journey
- Apertura: pico positivo (KPIs verdes, pill ok). Chips ⚠ generan ansiedad pero ofrecen remediio en 1 clic (bien).
- Captura larga: reconfortante (crono + ETA + fase + notificacion final); pero meseta al 95% (Math.min(95,...)) sin explicacion frustra.
- Error: valle. #st es una linea de 11px, truncada a 180 chars a mitad de palabra, con \n colapsados (verifyBtn emite multiples lineas que no se ven). El usuario se siente culpado, no ayudado.

## Red flags por persona
Alex (power user diario): cero atajos de teclado; Esc no cierra #modalOverlay; #moreMenu NO se deshabilita durante captura (puede lanzar reparse/upload en paralelo mientras #cap si se deshabilita); handlePreview usa prompt() para teclear el mes en formato YYYY-MM pese a existir el select #impMonthSel.
Jordan (primerizo): #uploadBtn ("Subir a Supabase", accion del flujo diario) enterrado en #moreMenu; mensaje "Lock huerfano - use Limpiar cache o detener" (STALE_LOCK) ininteligible; ⚙ #cfgBtn sin etiqueta; "Sufijos separados por coma" y "1277 = 3.5 anos" en config sin explicacion. Empty state (📦) si es bueno.

## Fortalezas
1. Maquinaria de estado: fase + % + cronometro + ETA + Notification de fin - rigor raro en tool interna.
2. Drag-drop de XLS en cualquier parte de la ventana abre modal con preview validado ANTES de insertar (stageFile -> preview_import).
3. Chips de dias faltantes convierten un fallo silencioso en remediacion de 1 clic; estimacion honesta >10 min con confirm.

## Issues priorizados
P1 - Recuperacion de errores rota: #st una linea, 180 chars, errores crudos. Fix: diccionario error->mensaje humano+accion; white-space:pre-line y mas tamano. ($impeccable clarify)
P1 - "Subir a Supabase" oculto: fila de 2 acciones primarias (Sincronizar | Subir), demitir el resto. ($impeccable distill)
P1 - 6 superficies de estado contradictorias (widgetAlert, statusPill, #st, #est, currentItem, dot). Consolidar en pill + una linea. ($impeccable quieter)
P2 - #moreMenu activo durante captura: deshabilitar con #cap. ($impeccable harden)
P2 - Cero teclado, confirm/prompt nativos bloqueantes: Esc en modales, select de mes en vez de prompt. ($impeccable harden)
P3 - Tipografia 9-11px + Inter (detector L22) + glows verdes (L28) + transition:width (L42). ($impeccable typeset)

## Menores
- updateTaskFlow mapea badges por regex sobre el texto del mensaje: fragil.
- utcToPeru hardcodea UTC-5 (ok hoy, fragil).
- verifyBtn une issues con \n que el status no puede mostrar.
- task-flow "captura/proceso/local" omite el paso Supabase.
- Footer "powered by g360" sin valor; "escritura (service role)" es info de dev.

## Preguntas provocadoras
- Si la tarea programada ya corre todo a las 12:00 y 20:00 sin abrir la app, no es esto un MONITOR, no un panel de control? Que pasaria si al abrir solo dijera "todo al dia" y escondiera todos los botones hasta que algo falle?
- Sobreviviria esta UI dividida en 3 vistas (Hoy / Reparar / Ajustes) en vez de 970 lineas apiladas?
- Que pasa si los errores llegaran a un panel de log consultable en vez de un ticker de 180 caracteres?
