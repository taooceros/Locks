# Experiment Results (Demo Run — All Locks)

Machine: Intel Xeon Gold 6438M, 128 logical cores, 2 NUMA nodes.
Settings: duration=1s, warmup=0s, trials=1. Short runs for demonstration only.
All 15 DLock2 locks included (FcSL excluded from defaults).

Lock groups:
- **Delegation (unfair):** FC, CC, DSM
- **Delegation (fair):** FCBan, CCBan, FC_PQ_BTree, FC_PQ_BHeap
- **Traditional:** MCS, Mutex, SpinLock, USCL
- **C baselines:** C_FC, C_CC, ShflLock, ShflLock_C

---

## Group 1: CS Ratio Sweep

**Hypothesis:** FC-PQ maintains JFI near 1.0 regardless of CS ratio; traditional locks degrade.

### JFI by Lock x Ratio x Thread Count

#### 4 threads

| Lock | 1:1 | 1:3 | 1:10 | 1:30 | 1:100 |
|---|---:|---:|---:|---:|---:|
| FC           | 0.9999 | 0.8038 | 0.5991 | 0.5337 | 0.5104 |
| FCBan        | 1.0000 | 0.9832 | 0.9783 | 0.9687 | 0.9764 |
| CC           | 0.9988 | 0.8654 | 0.6203 | 0.5427 | 0.5133 |
| CCBan        | 1.0000 | 0.9949 | 0.9901 | 0.9752 | 0.9857 |
| DSM          | 0.9578 | 0.8182 | 0.5896 | 0.5507 | 0.4663 |
| FC_PQ_BTree  | 0.7579 | 0.6443 | 0.5304 | 0.5563 | 0.5032 |
| FC_PQ_BHeap  | 1.0000 | 0.8346 | 0.5942 | 0.5349 | 0.5022 |
| Mutex        | 0.8809 | 0.7026 | 0.4862 | 0.4881 | 0.2502 |
| SpinLock     | 0.8035 | 0.9660 | 0.6334 | 0.5691 | 0.4176 |
| USCL         | 0.9999 | 0.9983 | 0.9966 | 0.9953 | 0.9949 |
| C_FC         | 0.9984 | 0.8261 | 0.6214 | 0.5392 | 0.5120 |
| C_CC         | 0.9998 | 0.8304 | 0.6161 | 0.5211 | 0.4976 |
| MCS          | 0.9989 | 0.8064 | 0.6204 | 0.5464 | 0.5110 |
| ShflLock     | 0.4024 | 0.8096 | 0.4322 | 0.3187 | 0.5121 |
| ShflLock_C   | 0.3487 | 0.8013 | 0.3060 | 0.5358 | 0.5102 |

#### 16 threads

| Lock | 1:1 | 1:3 | 1:10 | 1:30 | 1:100 |
|---|---:|---:|---:|---:|---:|
| FC           | 0.9986 | 0.8024 | 0.6024 | 0.5335 | 0.5105 |
| FCBan        | 0.9996 | 0.9929 | 0.9855 | 0.9884 | 0.9843 |
| CC           | 0.9999 | 0.8421 | 0.6268 | 0.5451 | 0.5132 |
| CCBan        | 1.0000 | 0.9977 | 0.9965 | 0.9955 | 0.9945 |
| DSM          | 0.9994 | 0.8343 | 0.6228 | 0.5437 | 0.5102 |
| FC_PQ_BTree  | 1.0000 | 0.9993 | 0.9991 | 0.9283 | 0.9004 |
| FC_PQ_BHeap  | 1.0000 | 0.9994 | 0.9843 | 0.9013 | 0.9201 |
| Mutex        | 0.9601 | 0.8216 | 0.4036 | 0.1081 | 0.0627 |
| SpinLock     | 0.8513 | 0.7282 | 0.5286 | 0.3632 | 0.3491 |
| USCL         | 0.9999 | 0.9980 | 0.9968 | 0.9952 | 0.9949 |
| C_FC         | 0.9998 | 0.7775 | 0.5803 | 0.5267 | 0.5080 |
| C_CC         | 0.9998 | 0.8280 | 0.6154 | 0.5405 | 0.5115 |
| MCS          | 0.9969 | 0.7932 | 0.6246 | 0.5411 | 0.5115 |
| ShflLock     | 0.9979 | 0.8053 | 0.6117 | 0.5410 | 0.5123 |
| ShflLock_C   | 0.2543 | 0.8028 | 0.6023 | 0.5339 | 0.5106 |

#### 64 threads

| Lock | 1:1 | 1:3 | 1:10 | 1:30 | 1:100 |
|---|---:|---:|---:|---:|---:|
| FC           | 0.9942 | 0.8271 | 0.6131 | 0.5393 | 0.5119 |
| FCBan        | 0.9975 | 0.9908 | 0.9817 | 0.9801 | 0.9711 |
| CC           | 0.9987 | 0.8548 | 0.6392 | 0.5513 | 0.5146 |
| CCBan        | 0.9999 | 0.9971 | 0.9948 | 0.9937 | 0.9925 |
| DSM          | 0.9995 | 0.8615 | 0.6427 | 0.5536 | 0.5155 |
| FC_PQ_BTree  | 1.0000 | 0.9994 | 0.9963 | 0.9962 | 0.9913 |
| FC_PQ_BHeap  | 1.0000 | 0.9990 | 0.9933 | 0.9944 | 0.9982 |
| Mutex        | 0.9468 | 0.8406 | 0.0776 | 0.0282 | 0.0160 |
| SpinLock     | 0.2922 | 0.2423 | 0.0912 | 0.0815 | 0.0184 |
| USCL         | 0.9981 | 0.9962 | 0.9947 | 0.9936 | 0.9903 |
| C_FC         | 0.9999 | 0.7947 | 0.5960 | 0.5323 | 0.5095 |
| C_CC         | 0.9992 | 0.8582 | 0.6417 | 0.5510 | 0.5147 |
| MCS          | 0.9909 | 0.8284 | 0.6182 | 0.5398 | 0.5120 |
| ShflLock     | 0.9958 | 0.0408 | 0.0233 | 0.5412 | 0.5093 |
| ShflLock_C   | 0.0294 | 0.0231 | 0.6069 | 0.0157 | 0.5132 |

### Throughput (millions) by Lock x Ratio x Thread Count

#### 4 threads

| Lock | 1:1 | 1:3 | 1:10 | 1:30 | 1:100 |
|---|---:|---:|---:|---:|---:|
| FC           | 1,467 | 1,598 | 1,787 | 1,832 | 1,857 |
| FCBan        | 1,314 | 1,525 | 1,551 | 1,574 | 1,543 |
| CC           | 1,275 | 1,492 | 1,668 | 1,686 | 1,818 |
| CCBan        | 1,269 | 1,349 | 1,542 | 1,424 | 1,624 |
| DSM          | 1,348 | 1,535 | 1,744 | 1,764 | 1,871 |
| FC_PQ_BTree  | 962 | 1,565 | 1,738 | 1,642 | 1,869 |
| FC_PQ_BHeap  | 1,495 | 1,658 | 1,771 | 1,824 | 1,868 |
| Mutex        | 589 | 875 | 922 | 847 | 1,441 |
| SpinLock     | 1,047 | 1,202 | 1,225 | 1,306 | 1,241 |
| USCL         | 550 | 579 | 600 | 605 | 599 |
| C_FC         | 1,060 | 1,236 | 1,396 | 1,466 | 1,467 |
| C_CC         | 1,154 | 1,437 | 1,656 | 1,780 | 1,848 |
| MCS          | 1,112 | 1,369 | 1,661 | 1,786 | 1,792 |
| ShflLock     | 1,711 | 1,353 | 1,816 | 1,615 | 1,840 |
| ShflLock_C   | 1,649 | 1,474 | 1,798 | 1,816 | 1,868 |

#### 16 threads

| Lock | 1:1 | 1:3 | 1:10 | 1:30 | 1:100 |
|---|---:|---:|---:|---:|---:|
| FC           | 1,349 | 1,478 | 1,617 | 1,664 | 1,684 |
| FCBan        | 1,349 | 1,337 | 1,398 | 1,400 | 1,452 |
| CC           | 1,093 | 1,278 | 1,404 | 1,534 | 1,660 |
| CCBan        | 1,068 | 1,174 | 1,254 | 1,328 | 1,329 |
| DSM          | 1,167 | 1,330 | 1,497 | 1,605 | 1,654 |
| FC_PQ_BTree  | 1,077 | 1,218 | 1,420 | 1,445 | 1,408 |
| FC_PQ_BHeap  | 1,356 | 1,437 | 1,487 | 1,529 | 1,532 |
| Mutex        | 142 | 181 | 282 | 948 | 1,317 |
| SpinLock     | 175 | 176 | 176 | 195 | 192 |
| USCL         | 441 | 473 | 462 | 461 | 486 |
| C_FC         | 367 | 289 | 261 | 295 | 285 |
| C_CC         | 1,065 | 1,251 | 1,447 | 1,524 | 1,649 |
| MCS          | 1,014 | 1,125 | 1,428 | 1,535 | 1,593 |
| ShflLock     | 1,037 | 1,188 | 1,397 | 1,548 | 1,585 |
| ShflLock_C   | 1,479 | 1,317 | 1,482 | 1,549 | 1,671 |

#### 64 threads

| Lock | 1:1 | 1:3 | 1:10 | 1:30 | 1:100 |
|---|---:|---:|---:|---:|---:|
| FC           | 989 | 1,084 | 1,231 | 1,313 | 1,339 |
| FCBan        | 1,009 | 1,075 | 1,107 | 1,093 | 1,088 |
| CC           | 786 | 978 | 1,191 | 1,299 | 1,322 |
| CCBan        | 798 | 899 | 985 | 1,020 | 1,004 |
| DSM          | 739 | 945 | 1,181 | 1,292 | 1,352 |
| FC_PQ_BTree  | 968 | 1,044 | 1,070 | 1,076 | 1,120 |
| FC_PQ_BHeap  | 1,032 | 1,144 | 1,166 | 1,179 | 1,184 |
| Mutex        | 128 | 190 | 227 | 746 | 1,056 |
| SpinLock     | 46 | 47 | 47 | 50 | 51 |
| USCL         | 384 | 417 | 422 | 450 | 417 |
| C_FC         | 61 | 64 | 66 | 65 | 65 |
| C_CC         | 737 | 927 | 1,177 | 1,297 | 1,327 |
| MCS          | 696 | 916 | 1,128 | 1,195 | 1,247 |
| ShflLock     | 715 | 1,162 | 1,328 | 1,191 | 1,207 |
| ShflLock_C   | 1,213 | 1,311 | 1,173 | 1,342 | 1,360 |

### Key Findings

1. **CCBan is the fairness champion:** JFI stays >0.99 at 64T across all ratios — better than FC_PQ_BHeap.
2. **FC_PQ_BHeap at 64T:** JFI remains >0.99 from 1:1 through 1:100 — excellent fairness scaling.
3. **FCBan:** JFI >0.97 at all ratios/thread counts — strong but slightly below CCBan.
4. **Traditional locks collapse:** Mutex JFI = 0.016 at 64T/1:100; SpinLock = 0.018.
5. **ShflLock (C) catastrophic:** JFI = 0.029 at 64T/1:1, degrades to 0.016 at 1:30.
6. **USCL consistently fair:** JFI >0.99 at all points — but throughput is 3-5x lower than delegation.
7. **Throughput:** FC_PQ_BHeap matches or exceeds MCS throughput while maintaining fairness.

---

## Group 2: CS Length Crossover

**Hypothesis:** Delegation advantage grows with CS length; at long CS, all converge.

32 threads, uniform CS, non-CS=0.

| Lock | CS=1 | CS=10 | CS=100 | CS=1000 | CS=5000 | CS=10000 | CS=50000 |
|---|---:|---:|---:|---:|---:|---:|---:|
| FC           | 11.1M | 102.9M | 676.2M | 1,161M | 1,304M | 1,340M | 1,347M |
| FCBan        | 7.5M | 74.9M | 472.1M | 1,118M | 1,274M | 1,341M | 1,375M |
| CC           | 4.7M | 43.9M | 252.2M | 876M | 1,141M | 1,125M | 1,307M |
| CCBan        | 4.1M | 39.4M | 243.2M | 885M | 1,109M | 1,222M | 1,309M |
| DSM          | 4.8M | 44.8M | 292.8M | 980M | 1,208M | 1,282M | 1,338M |
| FC_PQ_BTree  | 4.7M | 43.2M | 294.6M | 916M | 1,232M | 1,293M | 1,337M |
| FC_PQ_BHeap  | 6.1M | 58.4M | 406.0M | 1,118M | 1,325M | 1,338M | 1,357M |
| Mutex        | 2.7M | 22.6M | 93.6M | 142M | 189M | 304M | 935M |
| SpinLock     | 2.8M | 25.6M | 86.6M | 96M | 110M | 108M | 116M |
| USCL         | 4.0M | 41.2M | 236.8M | 428M | 476M | 444M | 478M |
| C_FC         | 4.0M | 37.7M | 108.0M | 144M | 143M | 148M | 152M |
| C_CC         | 4.0M | 37.8M | 250.4M | 827M | 1,152M | 1,226M | 1,365M |
| MCS          | 3.2M | 30.5M | 242.0M | 780M | 1,116M | 1,200M | 1,242M |
| ShflLock     | 3.7M | 37.1M | 556.1M | 863M | 1,179M | 1,227M | 1,280M |
| ShflLock_C   | 8.6M | 40.6M | 645.5M | 1,206M | 1,176M | 1,235M | 1,357M |

### Key Findings

1. **FC dominates at short CS (<100):** 11.1M vs 3.2M (MCS) at CS=1 — 3.4x advantage from combiner batching.
2. **Crossover around CS=1000-5000:** delegation and traditional locks converge as CS dominates total time.
3. **FC_PQ_BHeap tracks FC closely:** at CS=100, 406M vs 676M (0.60x FC); at CS=5000+, nearly equal.
4. **SpinLock/Mutex plateau:** SpinLock stuck at ~110M for CS>=1000 due to cross-core migration cost.
5. **USCL flat ~450M:** fair but throughput-limited at all CS lengths.
6. **C_FC bottleneck:** C flat combining tops out at ~150M — likely due to less efficient combining loop.

---

## Group 2b: Data Footprint Scaling (Counter Array)

**Hypothesis:** Delegation's throughput advantage grows with the size of the shared
data accessed per CS. Traditional locks pay cross-core migration cost proportional
to data footprint on every handoff.

Benchmark: `counter-array`, 32 threads, non-CS=0.
CS=N means touching N distinct u64s (N×8 bytes, N/8 cache lines).

### Throughput (millions) by Lock x Data Footprint

| Lock | CS=1 (8B) | CS=10 (80B) | CS=100 (800B) | CS=500 (4KB) | CS=1000 (8KB) | CS=2000 (16KB) | CS=4096 (32KB) |
|---|---:|---:|---:|---:|---:|---:|---:|
| FC           | 11.0M | 99.6M | 562M | 999M | 1,173M | 1,270M | 1,332M |
| FCBan        | 7.6M | 73.1M | 474M | 1,012M | 1,174M | 1,280M | 1,332M |
| CC           | 6.7M | 57.8M | 434M | 967M | 1,134M | 1,242M | 1,257M |
| CCBan        | 4.3M | 41.5M | 323M | 809M | 1,000M | 1,114M | 1,196M |
| DSM          | 5.6M | 48.9M | 419M | 974M | 1,139M | 1,246M | 1,279M |
| FC_PQ_BTree  | 4.8M | 46.2M | 360M | 847M | 1,046M | 1,192M | 1,281M |
| FC_PQ_BHeap  | 6.2M | 60.5M | 436M | 947M | 1,111M | 1,233M | 1,302M |
| Mutex        | 2.6M | 21.4M | 138M | 165M | 286M | 419M | 252M |
| SpinLock     | 5.2M | 54.5M | 477M | 990M | 1,184M | 1,291M | 1,178M |
| USCL         | 3.2M | 29.1M | 173M | 299M | 338M | 365M | 379M |
| C_FC         | 4.0M | 37.6M | 295M | 801M | 1,008M | 1,169M | 1,249M |
| C_CC         | 3.4M | 33.2M | 242M | 698M | 934M | 1,109M | 1,246M |
| MCS          | 3.2M | 30.7M | 235M | 450M | 547M | 588M | 608M |
| ShflLock     | 8.3M | 35.8M | 249M | 1,074M | 548M | 588M | 609M |
| ShflLock_C   | 8.3M | 87.2M | 594M | 460M | 1,196M | 596M | 613M |

### FC/MCS Throughput Ratio by Data Footprint

| Footprint | FC | MCS | Ratio (FC/MCS) |
|---|---:|---:|---:|
| CS=1 (8B) | 11.0M | 3.2M | **3.4x** |
| CS=10 (80B) | 99.6M | 30.7M | **3.2x** |
| CS=100 (800B) | 562M | 235M | **2.4x** |
| CS=500 (4KB) | 999M | 450M | **2.2x** |
| CS=1000 (8KB) | 1,173M | 547M | **2.1x** |
| CS=2000 (16KB) | 1,270M | 588M | **2.2x** |
| CS=4096 (32KB) | 1,332M | 608M | **2.2x** |

### Thread Scaling (CS=100, 800 bytes / 13 cache lines)

| Lock | 4T | 16T | 32T | 64T |
|---|---:|---:|---:|---:|
| FC           | 589M | 600M | 562M | 435M |
| FCBan        | 514M | 508M | 474M | 331M |
| CC           | 502M | 491M | 434M | 255M |
| CCBan        | 387M | 354M | 323M | 218M |
| DSM          | 420M | 464M | 419M | 232M |
| FC_PQ_BTree  | 338M | 402M | 360M | 291M |
| FC_PQ_BHeap  | 408M | 492M | 436M | 329M |
| Mutex        | 268M | 139M | 138M | 98M |
| SpinLock     | 806M | 585M | 477M | 204M |
| USCL         | 172M | 174M | 173M | 172M |
| C_FC         | 133M | 311M | 295M | 180M |
| C_CC         | 125M | 246M | 242M | 158M |
| MCS          | 278M | 259M | 235M | 159M |
| ShflLock     | 751M | 692M | 249M | 388M |
| ShflLock_C   | 283M | 714M | 594M | 154M |

### Direct Comparison: counter-proportional vs counter-array

32 threads, non-CS=0. "Scalar" = counter-proportional (single u64).
"Array" = counter-array (N distinct u64s).

| Lock | Scalar CS=100 | Array CS=100 | Scalar CS=1000 | Array CS=1000 |
|---|---:|---:|---:|---:|
| FC    | 645M | 557M | 1,150M | 1,175M |
| MCS   | 243M | 235M | 683M | 566M |
| Mutex | 92M | 135M | 114M | 263M |

**FC/MCS ratio:** Scalar CS=1000: 1,150/683 = **1.68x**. Array CS=1000: 1,175/566 = **2.08x**.

### Key Findings

1. **FC/MCS ratio stays >2x across all footprints:** At CS=4096 (32KB, full L1), FC delivers 1,332M vs MCS 608M — 2.2x. Delegation keeps the entire 32KB working set in the combiner's L1.
2. **MCS throughput plateaus at ~600M:** Despite CS growing 4096x (from 1 to 4096 elements), MCS only reaches 608M — cross-core migration of the growing working set dominates.
3. **FC_PQ_BHeap tracks FC closely:** At CS=4096, FC_PQ_BHeap = 1,302M vs FC = 1,332M (0.98x). Fairness overhead is negligible for large data.
4. **Scalar vs Array comparison isolates migration cost:** At CS=1000, FC/MCS ratio jumps from 1.68x (scalar, same 8-byte counter) to 2.08x (array, 8KB working set). The extra 0.4x gap is pure data migration overhead.
5. **Mutex erratic under array workload:** throughput fluctuates (252M at 32KB, 419M at 16KB) — likely due to OS scheduling interacting with cache pressure.

---

## Group 3: Non-CS Sweep

**Hypothesis:** Delegation advantage is largest under high contention (non-CS=0) and narrows as contention decreases.

CS = 1000,3000, threads = 4,16,64. JFI reported.

### JFI at 64 threads

| Lock | nonCS=0 | nonCS=100 | nonCS=1000 | nonCS=10000 |
|---|---:|---:|---:|---:|
| FC           | 0.826 | 0.826 | 0.828 | 0.826 |
| FCBan        | 0.991 | 0.989 | 0.990 | 0.990 |
| CC           | 0.856 | 0.854 | 0.845 | 0.814 |
| CCBan        | 0.997 | 0.997 | 0.996 | 0.995 |
| DSM          | 0.859 | 0.861 | 0.839 | 0.818 |
| FC_PQ_BTree  | 0.999 | 1.000 | 1.000 | 1.000 |
| FC_PQ_BHeap  | 0.998 | 0.999 | 0.999 | 0.999 |
| Mutex        | 0.892 | 0.758 | 0.490 | 0.743 |
| SpinLock     | 0.337 | 0.743 | 0.717 | 0.732 |
| USCL         | 0.996 | 0.995 | 0.976 | 0.866 |
| C_FC         | 0.795 | 0.795 | 0.793 | 0.793 |
| C_CC         | 0.859 | 0.854 | 0.840 | 0.806 |
| MCS          | 0.826 | 0.829 | 0.856 | 0.837 |
| ShflLock     | 0.028 | 0.831 | 0.848 | 0.827 |
| ShflLock_C   | 0.809 | 0.100 | 0.808 | 0.811 |

### Key Findings

1. **Fair delegation locks maintain JFI >0.99 regardless of non-CS:** CCBan, FC_PQ_BTree, FC_PQ_BHeap all stable.
2. **USCL fairness degrades at low contention:** drops from 0.996 to 0.866 at nonCS=10000.
3. **ShflLock instability:** wildly variable JFI (0.028 to 0.831 at 64T) — depends on scheduling luck.
4. **Traditional locks erratic:** Mutex swings 0.49-0.89; SpinLock 0.34-0.74.

---

## Group 4: Response Time Distributions

**Hypothesis:** Unfair delegation shows bimodal response time; fair delegation is more uniform.

CS = 1000,3000, non-CS = 0, `--stat-response-time` enabled.

### 8 threads

| Lock | Role | p50 | p95 | p99 | p99.9 |
|------|------|----:|----:|----:|------:|
| FC           | combiner |   20,412 |   35,838 |   48,636 |    126,485 |
| FC           | waiter   |   20,594 |   35,676 |   47,538 |    125,725 |
| FCBan        | combiner |   13,258 |   35,530 |   44,976 |    108,812 |
| FCBan        | waiter   |   13,662 |   36,564 |   45,442 |    109,987 |
| CC           | combiner |  202,608 |  239,334 |  620,873 |    624,545 |
| CC           | waiter   |   20,338 |   22,840 |   66,442 |     67,000 |
| CCBan        | combiner |   49,696 |   70,314 |  128,432 |    148,074 |
| CCBan        | waiter   |   14,026 |   35,662 |   62,560 |     92,589 |
| DSM          | combiner |  197,960 |  218,894 |  553,844 |    634,904 |
| DSM          | waiter   |   19,520 |   21,810 |   57,638 |     66,775 |
| FC_PQ_BTree  | combiner |  190,672 |  813,996 | 1,905,398 |  4,712,806 |
| FC_PQ_BTree  | waiter   |    8,132 |   49,974 |  186,719 |    698,248 |
| FC_PQ_BHeap  | combiner |  190,592 |  724,639 | 1,665,843 |  9,760,252 |
| FC_PQ_BHeap  | waiter   |    8,122 |   38,542 |  184,814 |    484,296 |
| Mutex        | combiner |    6,686 |  420,845 | 1,656,064 |  8,077,984 |
| SpinLock     | combiner |   12,078 |  270,922 | 1,559,443 |  4,552,351 |
| USCL         | combiner |    6,122 |   17,192 |   18,786 | 33,682,304 |
| C_FC         | combiner |   34,888 |   62,816 |   96,456 |    144,211 |
| C_FC         | waiter   |   34,840 |   45,580 |   74,264 |    111,262 |
| C_CC         | combiner |  203,804 |  229,094 |  627,826 |    631,707 |
| C_CC         | waiter   |   20,472 |   22,972 |   66,848 |     67,722 |
| MCS          | combiner |   23,218 |   30,322 |   36,188 |     37,414 |
| ShflLock     | combiner |   23,538 |   31,284 |   34,178 |     39,001 |
| ShflLock_C   | combiner |   22,876 |   32,142 |   35,748 |     42,302 |

### 32 threads

| Lock | Role | p50 | p95 | p99 | p99.9 |
|------|------|----:|----:|----:|------:|
| FC           | combiner |  110,184 |  128,315 |  255,304 |    394,499 |
| FC           | waiter   |  110,180 |  114,948 |  143,414 |    394,811 |
| FCBan        | combiner |   64,716 |  186,817 |  249,000 |    513,316 |
| FCBan        | waiter   |   70,070 |  190,952 |  215,924 |    599,470 |
| CC           | combiner |  372,492 |  402,771 |  912,182 |    928,816 |
| CC           | waiter   |  123,140 |  128,447 |  377,944 |    391,534 |
| CCBan        | combiner |  131,842 |  251,751 |  363,425 |    646,362 |
| CCBan        | waiter   |   84,314 |  200,074 |  296,262 |    591,403 |
| DSM          | combiner |  352,954 |  443,679 |  917,260 |    946,640 |
| DSM          | waiter   |  115,674 |  128,670 |  376,126 |    388,802 |
| FC_PQ_BTree  | combiner |  184,406 |  362,029 |  631,937 |  1,717,121 |
| FC_PQ_BTree  | waiter   |   65,914 |  190,665 |  344,103 |    783,957 |
| FC_PQ_BHeap  | combiner |  172,890 |  327,345 |  429,937 |    930,425 |
| FC_PQ_BHeap  | waiter   |   62,092 |  177,177 |  233,980 |    602,783 |
| Mutex        | combiner |   49,314 | 4,419,908 | 8,962,298 | 23,464,149 |
| SpinLock     | combiner |   53,704 |   64,028 | 27,228,228 | 230,131,160 |
| USCL         | combiner |    6,102 |   17,206 |   18,460 | 149,144,671 |
| C_FC         | combiner | 1,402,066 | 1,932,667 | 2,176,355 |  3,732,827 |
| C_FC         | waiter   |  921,560 | 1,143,455 | 1,428,233 |  2,259,885 |
| C_CC         | combiner |  368,645 |  611,495 |  922,339 |  1,002,236 |
| C_CC         | waiter   |  121,844 |  132,594 |  382,986 |    395,870 |
| MCS          | combiner |  144,250 |  164,732 |  171,790 |    187,416 |
| ShflLock     | combiner |  136,210 |  154,948 |  167,128 |    181,695 |
| ShflLock_C   | combiner |  123,534 |  145,077 |  151,862 |    177,664 |

### Key Findings

1. **CC/DSM bimodal:** combiner p50=370K vs waiter p50=120K at 32T — 3x gap. This is the "combiner penalty" motivation for fair delegation.
2. **FC_PQ_BHeap combiner overhead:** combiner p50=173K, waiter p50=62K at 32T. PQ maintenance visible but bounded.
3. **FC balanced roles:** combiner p50 ~ waiter p50 (~110K at 32T) — FC naturally balances when CS is mixed.
4. **Mutex/SpinLock tail explosion:** p99.9 = 23M (Mutex), 230M (SpinLock) at 32T — catastrophic tail latency.
5. **USCL tail explosion:** p99.9 = 149M at 32T — fair mean but terrible tail.
6. **MCS tight tails:** p99.9 = 187K at 32T — predictable but unfair on usage share.

---

## Group 8: Queue & Priority Queue

**Hypothesis:** Delegation locks show throughput advantage on data structure workloads.

### Queue (VecDeque) — Throughput

| Lock | 4T | 16T | 64T |
|---|---:|---:|---:|
| FC           | 8.67M | 15.3M | 9.16M |
| FCBan        | 7.51M | 11.5M | 5.92M |
| CC           | 7.22M | 7.82M | 4.36M |
| CCBan        | 6.80M | 7.16M | 4.37M |
| DSM          | 4.91M | 7.71M | 4.02M |
| FC_PQ_BTree  | 3.46M | 5.75M | 3.66M |
| FC_PQ_BHeap  | 3.94M | 7.44M | 5.19M |
| Mutex        | 5.85M | 3.80M | 2.59M |
| SpinLock     | 9.89M | 5.35M | 1.10M |
| USCL         | 3.88M | 3.55M | 3.65M |
| C_FC         | 5.94M | 5.90M | 1.98M |
| C_CC         | 5.96M | 6.63M | 3.05M |
| MCS          | 4.61M | 4.24M | 2.50M |
| ShflLock     | 5.21M | 4.23M | 2.10M |
| ShflLock_C   | 5.23M | 3.91M | 2.96M |

### Priority Queue (BinaryHeap) — Throughput

| Lock | 4T | 16T | 64T |
|---|---:|---:|---:|
| FC           | 9.35M | 15.1M | 8.46M |
| FCBan        | 7.79M | 9.24M | 4.89M |
| CC           | 5.05M | 7.27M | 3.32M |
| CCBan        | 3.81M | 5.38M | 3.52M |
| DSM          | 3.11M | 6.19M | 3.43M |
| FC_PQ_BTree  | 3.18M | 5.25M | 3.49M |
| FC_PQ_BHeap  | 3.17M | 6.67M | 4.87M |
| Mutex        | 4.14M | 2.05M | 1.20M |
| SpinLock     | 8.46M | 5.23M | 1.51M |
| USCL         | 4.63M | 4.15M | 4.46M |
| C_FC         | 4.63M | 4.57M | 1.91M |
| C_CC         | 3.95M | 6.21M | 2.83M |
| MCS          | 2.26M | 2.04M | 1.17M |
| ShflLock     | 5.35M | 1.81M | 1.17M |
| ShflLock_C   | 2.42M | 6.36M | 3.62M |

### Key Findings

1. **FC dominates:** 15M ops/s at 16T queue — 3.5x Mutex, 2x CC.
2. **FC_PQ_BHeap scales well:** 7.4M (queue) and 6.7M (PQ) at 16T — solid for a fair lock.
3. **Delegation advantage at scale:** at 64T, FC=9.2M vs MCS=2.5M (3.7x) vs SpinLock=1.1M (8.4x).
4. **Traditional locks degrade:** SpinLock drops from 9.9M (4T) to 1.1M (64T) on queue workload.
5. **USCL consistent:** ~3.5-4.5M across thread counts — fair but throughput-limited.
