class Main {
    int breakOuter(int[] values) {
        outer:
        for (int v : values) {
            while (true) {
                if (v > 0) {
                    break outer;
                }
                return v;
            }
        }
    /*@break_outer*/}

    int labeledBlock(boolean flag) {
        done: {
            if (flag) {
                break done;
            }
            return 1;
        }
    /*@labeled_block*/}

    int breakLabeledLoop(int n) {
        loop:
        while (true) {
            if (n > 3) {
                break loop;
            }
            n++;
        }
    /*@labeled_loop*/}

    void unreachable() {
        while (true) {
            inner: {
                break inner;
            }
        }
        /*@after*/System.out.println();
    }

    int ok1(int[] values) {
        while (true) {
            inner: {
                if (values.length > 0) {
                    break inner;
                }
            }
        }
    }

    int ok2(int[][] grid) {
        outer:
        while (true) {
            for (int[] row : grid) {
                if (row.length == 0) {
                    continue outer;
                }
                return row[0];
            }
        }
    }

    int ok3(boolean flag) {
        loop:
        do {
            if (flag) {
                continue loop;
            }
            return 1;
        } while (true);
    }

    int ok4(int n) {
        outer:
        for (;;) {
            try {
                if (n > 0) {
                    break outer;
                }
            } finally {
                return n;
            }
        }
    }

    int ok5(int[] values) {
        int total = 0;
        outer:
        for (int v : values) {
            for (int i = 0; i < v; i++) {
                if (i > 5) {
                    continue outer;
                }
                if (i == v - 1) {
                    break outer;
                }
                total += i;
            }
        }
        return total;
    }

    void ok6(boolean flag) {
        outer:
        while (true) {
            while (true) {
                if (flag) {
                    break outer;
                }
            }
        }
        System.out.println();
    }
}
//? diag missing-return break_outer
//? diag missing-return labeled_block
//? diag missing-return labeled_loop
//? diag unreachable-code after
//? diag exact
