class Main {
    int afterReturn() {
        return 1;
        /*@after_return*/int x = 2;
    }

    void afterThrow() {
        throw new IllegalStateException();
        /*@after_throw*/System.out.println();
    }

    void afterBreak() {
        while (true) {
            break;
            /*@after_break*/System.out.println();
        }
    }

    void afterContinue(int n) {
        for (int i = 0; i < n; i++) {
            continue;
            /*@after_continue*/System.out.println();
        }
    }

    void afterInfiniteLoop() {
        while (true) {
        }
        /*@after_loop*/System.out.println();
    }

    void afterForEver() {
        for (;;) {
        }
        /*@after_for*/System.out.println();
    }

    int reachable(boolean flag, int n) {
        if (flag) {
            return 1;
        }
        while (n > 0) {
            n--;
            if (n == 3) break;
        }
        if (false) {
            n++;
        }
        do {
            n++;
        } while (n < 10);
        try {
            return n;
        } finally {
            System.out.println();
        }
    }
}
//? diag unreachable-code after_return
//? diag unreachable-code after_throw
//? diag unreachable-code after_break
//? diag unreachable-code after_continue
//? diag unreachable-code after_loop
//? diag unreachable-code after_for
//? diag exact
