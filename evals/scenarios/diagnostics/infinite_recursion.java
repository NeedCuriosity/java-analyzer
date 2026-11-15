import java.util.List;

class Main {
    private String name;
    private int count;

    void setName(String name) {
        /*@setter*/setName(name);
    }

    int getCount() {
        return this./*@getter*/getCount();
    }

    int depth(List<String> items) {
        int n = items.size() + /*@expr*/depth(items);
        return n;
    }

    int factorial(int n) {
        if (n <= 1) {
            return 1;
        }
        return n * factorial(n - 1);
    }

    int guarded(int n) {
        return n > 0 ? guarded(n - 1) : 0;
    }

    boolean shortCircuit(int n) {
        return n > 0 && shortCircuit(n - 1);
    }

    void other(Main next) {
        next.other(this);
    }

    Runnable deferred() {
        Runnable r = () -> deferred();
        return r;
    }

    void loop(List<String> items) {
        for (String item : items) {
            loop(items);
        }
    }

    String describe() {
        System.out.println(name);
        return /*@late*/describe() + count;
    }
}
//? diag infinite-recursion setter
//? diag infinite-recursion getter
//? diag infinite-recursion expr
//? diag infinite-recursion late
//? diag only infinite-recursion
