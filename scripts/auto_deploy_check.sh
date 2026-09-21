#!/bin/bash
# auto_deploy_check.sh
# Linux/Mac version

echo "=== VERIFICACIÓN DE DEPLOY ==="

# 1. Verificar credenciales
CONFIG="$HOME/.config/g360-db-ventas/data/config.json"
if [ -f "$CONFIG" ]; then
    if grep -q "TU_SUPABASE" "$CONFIG"; then
        echo "❌ ERROR: Supabase no configurado"
        exit 1
    fi
    echo "✅ Credenciales OK"
else
    echo "❌ ERROR: config.json no encontrado"
    exit 1
fi

# 2. Verificar BD
DB="$HOME/.config/g360-db-ventas/data/historial.db"
if [ -f "$DB" ]; then
    SIZE=$(du -m "$DB" | cut -f1)
    echo "✅ BD existe ($SIZE MB)"
else
    echo "ℹ️  BD no existe (primera vez) - se creará automáticamente"
fi

# 3. Verificar raw
RAW="$HOME/.config/g360-db-ventas/data/raw"
if [ -d "$RAW" ]; then
    CSV_COUNT=$(find "$RAW" -name "*.csv" | wc -l)
    echo "✅ Raw directory OK ($CSV_COUNT CSVs)"
else
    echo "ℹ️  Raw directory vacío (captura pendiente)"
fi

echo ""
echo "=== DEPLOY VERIFICADO ==="
echo "Listo para capturar. Rango recomendado: 2014-01 hasta hoy"
