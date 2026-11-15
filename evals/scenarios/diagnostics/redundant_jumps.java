import java.util.List;

class Main {
    void plain(List<String> items) {
        System.out.println(items);
        /*@ret*/return;
    }

    void branches(boolean flag) {
        if (flag) {
            System.out.println("a");
            /*@if_ret*/return;
        } else if (!flag) {
            /*@elif_ret*/return;
        }
    }

    void needed(boolean flag) {
        if (flag) {
            return;
        }
        System.out.println("b");
    }

    void loops(List<String> items) {
        for (String item : items) {
            if (item.isEmpty()) {
                /*@cont*/continue;
            }
        }
        for (String item : items) {
            if (item.isEmpty()) {
                continue;
            }
            System.out.println(item);
        }
        outer:
        for (String item : items) {
            for (String other : items) {
                continue outer;
            }
        }
    }

    void inTry() {
        try {
            System.out.println("c");
            return;
        } finally {
            System.out.println("d");
        }
    }

    void unbraced(boolean flag) {
        if (flag) return;
    }

    Runnable lambda() {
        return () -> {
            System.out.println("e");
            /*@lambda_ret*/return;
        };
    }
}
//? diag redundant-jump ret
//? diag redundant-jump if_ret
//? diag redundant-jump elif_ret
//? diag redundant-jump cont
//? diag redundant-jump lambda_ret
//? diag only redundant-jump
//? assist simplify ret ; has "        System.out.println(items);\n    }"
