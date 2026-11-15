class Main {
    int simple() {
        int x;
        return /*@simple*/x;
    }

    int oneBranch(boolean flag) {
        int x;
        if (flag) {
            x = 1;
        }
        return /*@one_branch*/x;
    }

    int inLoop(int n) {
        int x;
        while (n > 0) {
            x = n;
            n--;
        }
        return /*@in_loop*/x;
    }

    int compound() {
        int x;
        /*@compound*/x += 1;
        return x;
    }

    int switchMissing(int n) {
        int x;
        switch (n) {
            case 1: x = 1; break;
            case 2: x = 2; break;
        }
        return /*@switch_missing*/x;
    }

    int ok1(boolean flag) {
        int x;
        if (flag) x = 1; else x = 2;
        return x;
    }

    int ok2(boolean flag) {
        int x;
        if (flag) {
            x = 1;
        } else {
            throw new IllegalStateException();
        }
        return x;
    }

    int ok3(int n) {
        int x;
        switch (n) {
            case 1: x = 1; break;
            default: x = 0;
        }
        return x;
    }

    int ok4() {
        int x;
        try {
            x = Integer.parseInt("1");
        } catch (NumberFormatException e) {
            x = 0;
        }
        return x;
    }

    int ok5(boolean a, boolean b) {
        int x;
        if (a && (x = 1) > 0) {
            return x;
        }
        boolean y;
        if (b || (y = a)) {
            return 0;
        } else {
            return y ? 1 : 2;
        }
    }

    int ok6() {
        final int x;
        x = 3;
        Runnable r = () -> System.out.println(x);
        r.run();
        return x;
    }

    int ok7(int n) {
        int x;
        do {
            x = n--;
        } while (n > 0);
        return x;
    }

    int ok8(int n) {
        int x;
        for (;;) {
            if (n > 0) { x = n; break; }
            n++;
        }
        return x;
    }
}
//? diag uninitialized-variable simple
//? diag uninitialized-variable one_branch
//? diag uninitialized-variable in_loop
//? diag uninitialized-variable compound
//? diag uninitialized-variable switch_missing
//? diag exact
