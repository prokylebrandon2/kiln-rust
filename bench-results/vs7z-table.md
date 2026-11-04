| Dataset (raw) | fs | Tool | Size MB | Ratio | Create s | Create CPU s | Create RSS MiB | Test s | Extract s | Extract RSS MiB | n |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| d3_mixed (89 MB, 310 files) | ext4 | kiln fastest | 55.23 | 1.62 | 0.18 | 0.2 | 41 | 0.07 | 0.12 | 53 | 3 |
| d3_mixed (89 MB, 310 files) | ext4 | 7z -mx1 | 51.44 | 1.74 | 3.35 | 6.5 | 22 | 0.31 | 0.42 | 13 | 3 |
| d3_mixed (89 MB, 310 files) | ext4 | zip (7z, deflate -mx1) | 56.07 | 1.59 | 0.98 | 1.9 | 23 | 0.26 | 0.35 | 7 | 3 |
| d3_mixed (89 MB, 310 files) | ext4 | kiln balanced | 53.45 | 1.67 | 0.25 | 0.4 | 41 | 0.07 | 0.17 | 54 | 3 |
| d3_mixed (89 MB, 310 files) | ext4 | kiln adaptive | 53.12 | 1.68 | 0.31 | 0.3 | 33 | 0.07 | 0.15 | 59 | 3 |
| d3_mixed (89 MB, 310 files) | ext4 | 7z -mx5 (default) | 49.25 | 1.81 | 13.17 | 20.6 | 381 | 0.53 | 0.61 | 106 | 3 |
| d3_mixed (89 MB, 310 files) | ext4 | zip -6 (Info-ZIP) | 55.68 | 1.60 | 2.90 | 2.9 | 2 | 0.56 | 0.63 | 3 | 3 |
| d3_mixed (89 MB, 310 files) | ext4 | kiln maximum | 50.66 | 1.76 | 8.71 | 13.1 | 249 | 0.09 | 0.17 | 106 | 1 |
| d3_mixed (89 MB, 310 files) | ext4 | 7z -mx9 (ultra) | 48.92 | 1.83 | 17.63 | 26.0 | 603 | 0.52 | 0.61 | 136 | 1 |
| d5_random (268 MB, 64 files) | ext4 | kiln fastest | 268.44 | 1.00 | 0.46 | 0.4 | 40 | 0.07 | 1.61 | 24 | 3 |
| d5_random (268 MB, 64 files) | ext4 | 7z -mx1 | 268.45 | 1.00 | 16.84 | 32.9 | 16 | 0.09 | 0.86 | 11 | 3 |
| d5_random (268 MB, 64 files) | ext4 | zip (7z, deflate -mx1) | 268.44 | 1.00 | 3.66 | 6.9 | 26 | 0.11 | 0.21 | 6 | 3 |
| d5_random (268 MB, 64 files) | ext4 | kiln balanced | 268.44 | 1.00 | 0.45 | 0.3 | 42 | 0.07 | 1.52 | 24 | 3 |
| d5_random (268 MB, 64 files) | ext4 | kiln adaptive | 268.44 | 1.00 | 0.35 | 0.2 | 12 | 0.08 | 0.15 | 24 | 3 |
| d5_random (268 MB, 64 files) | ext4 | 7z -mx5 (default) | 268.45 | 1.00 | 31.75 | 52.0 | 381 | 0.28 | 1.43 | 167 | 3 |
| d5_random (268 MB, 64 files) | ext4 | zip -6 (Info-ZIP) | 268.49 | 1.00 | 7.25 | 7.2 | 2 | 1.42 | 2.05 | 3 | 3 |
| d5_random (268 MB, 64 files) | ext4 | kiln maximum | 268.44 | 1.00 | 0.36 | 0.2 | 68 | 0.10 | 1.88 | 84 | 1 |
| d5_random (268 MB, 64 files) | ext4 | 7z -mx9 (ultra) | 268.45 | 1.00 | 61.08 | 88.4 | 2571 | 0.49 | 0.65 | 518 | 1 |
| d1_pytree (131 MB, 8305 files) | ext4 | kiln fastest | 32.21 | 4.07 | 0.34 | 0.6 | 40 | 0.11 | 1.36 | 55 | 3 |
| d1_pytree (131 MB, 8305 files) | ext4 | 7z -mx1 | 26.28 | 4.99 | 3.02 | 5.3 | 33 | 0.56 | 3.18 | 12 | 3 |
| d1_pytree (131 MB, 8305 files) | ext4 | zip (7z, deflate -mx1) | 35.81 | 3.66 | 1.59 | 2.8 | 28 | 0.69 | 3.50 | 10 | 3 |
| d1_pytree (131 MB, 8305 files) | ext4 | kiln balanced | 28.98 | 4.53 | 0.48 | 0.8 | 37 | 0.12 | 2.47 | 54 | 3 |
| d1_pytree (131 MB, 8305 files) | ext4 | kiln adaptive | 28.98 | 4.53 | 0.54 | 0.9 | 37 | 0.12 | 2.71 | 55 | 3 |
| d1_pytree (131 MB, 8305 files) | ext4 | 7z -mx5 (default) | 20.42 | 6.43 | 64.55 | 80.9 | 390 | 1.05 | 4.14 | 152 | 3 |
| d1_pytree (131 MB, 8305 files) | ext4 | zip -6 (Info-ZIP) | 34.76 | 3.77 | 3.74 | 3.7 | 8 | 0.87 | 4.64 | 4 | 3 |
| d1_pytree (131 MB, 8305 files) | ext4 | kiln maximum | 21.41 | 6.13 | 37.20 | 72.3 | 260 | 0.15 | 2.70 | 114 | 1 |
| d1_pytree (131 MB, 8305 files) | ext4 | 7z -mx9 (ultra) | 19.35 | 6.78 | 91.45 | 112.5 | 1274 | 1.06 | 4.24 | 151 | 1 |
| d4_large.bin (403 MB, 1 files) | ext4 | kiln fastest | 168.33 | 2.39 | 0.68 | 1.0 | 42 | 0.25 | 2.43 | 37 | 3 |
| d4_large.bin (403 MB, 1 files) | ext4 | 7z -mx1 | 147.51 | 2.73 | 10.59 | 20.8 | 15 | 2.71 | 3.84 | 10 | 3 |
| d4_large.bin (403 MB, 1 files) | ext4 | zip (7z, deflate -mx1) | 185.80 | 2.17 | 9.08 | 9.0 | 11 | 2.16 | 2.89 | 7 | 3 |
| d4_large.bin (403 MB, 1 files) | ext4 | kiln balanced | 134.62 | 2.99 | 0.81 | 1.3 | 35 | 0.20 | 2.22 | 38 | 3 |
| d4_large.bin (403 MB, 1 files) | ext4 | kiln adaptive | 134.62 | 2.99 | 0.86 | 1.4 | 34 | 0.22 | 1.56 | 43 | 3 |
| d4_large.bin (403 MB, 1 files) | ext4 | 7z -mx5 (default) | 72.70 | 5.54 | 51.42 | 65.4 | 381 | 2.34 | 4.69 | 62 | 3 |
| d4_large.bin (403 MB, 1 files) | ext4 | zip -6 (Info-ZIP) | 184.47 | 2.18 | 15.25 | 15.2 | 2 | 2.89 | 3.82 | 3 | 3 |
| d2_tiny (93 MB, 30000 files) | tmpfs | kiln fastest | 29.11 | 3.19 | 0.50 | 0.8 | 47 | 0.13 | 0.48 | 58 | 3 |
| d2_tiny (93 MB, 30000 files) | tmpfs | 7z -mx1 | 27.45 | 3.38 | 5.64 | 8.8 | 49 | 0.52 | 0.84 | 15 | 3 |
| d2_tiny (93 MB, 30000 files) | tmpfs | zip (7z, deflate -mx1) | 42.89 | 2.16 | 2.44 | 3.8 | 48 | 0.83 | 1.35 | 20 | 3 |
| d2_tiny (93 MB, 30000 files) | tmpfs | kiln balanced | 27.87 | 3.33 | 0.51 | 0.8 | 48 | 0.14 | 0.40 | 53 | 3 |
| d2_tiny (93 MB, 30000 files) | tmpfs | kiln adaptive | 27.87 | 3.33 | 0.72 | 1.2 | 49 | 0.15 | 0.44 | 57 | 3 |
| d2_tiny (93 MB, 30000 files) | tmpfs | 7z -mx5 (default) | 23.03 | 4.03 | 128.27 | 150.2 | 409 | 1.66 | 2.21 | 121 | 3 |
| d2_tiny (93 MB, 30000 files) | tmpfs | zip -6 (Info-ZIP) | 40.75 | 2.28 | 3.38 | 3.3 | 21 | 0.86 | 1.57 | 5 | 3 |
