add-migration:
	sea migrate -d src/openwhoop-migration/ generate $(NAME)

run-migrations:
	sea migrate -d src/openwhoop-migration/
	sea generate entity --output-dir src/openwhoop-entities/src/ --lib

test-report:
	cargo llvm-cov --html --open

snoop-ble:
	adb shell "nc -s 127.0.0.1 -p 8872 -L system/bin/tail -f -c +0 data/misc/bluetooth/logs/btsnoop_hci.log"

# Launch the interactive dev dashboard (Marimo notebook).
# Requires: `pip install -r lab/requirements.txt` in a venv first.
# See lab/README.md for first-time setup.
lab:
	@if [ -d lab/.venv ]; then \
		. lab/.venv/bin/activate && marimo edit lab/dashboard.py; \
	else \
		echo "First-time setup needed. Run:"; \
		echo "  python -m venv lab/.venv"; \
		echo "  source lab/.venv/bin/activate"; \
		echo "  pip install -r lab/requirements.txt"; \
		echo "Then re-run: make lab"; \
		exit 1; \
	fi

lab-setup:
	python -m venv lab/.venv
	. lab/.venv/bin/activate && pip install -r lab/requirements.txt

.PHONY: add-migration run-migrations test-report snoop-ble lab lab-setup