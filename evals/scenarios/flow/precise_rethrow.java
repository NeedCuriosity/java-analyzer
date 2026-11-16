import java.io.IOException;

class Main {
    static class AEx extends Exception {}

    static class BEx extends Exception {}

    void a() throws AEx {}

    void b() throws BEx {}

    void io() throws IOException {}

    void rethrowAll() throws AEx {
        try {
            a();
            b();
        } catch (Exception e) {
            throw /*@precise*/e;
        }
    }

    void reassigned() throws AEx, BEx {
        try {
            a();
        } catch (Exception e) {
            e = new AEx();
            throw /*@reassigned*/e;
        }
    }

    void param(IOException failure) {
        throw /*@param*/failure;
    }

    void local() {
        BEx failure = new BEx();
        throw /*@local*/failure;
    }

    void multi() throws AEx {
        try {
            a();
            b();
        } catch (AEx | BEx e) {
            throw /*@multi*/e;
        }
    }

    void ok1() throws AEx {
        try {
            a();
        } catch (Exception e) {
            throw e;
        }
    }

    void ok2() throws BEx {
        try {
            a();
            b();
        } catch (AEx e) {
            System.out.println(e);
        } catch (Exception e) {
            throw e;
        }
    }

    void ok3() {
        try {
            System.out.println("x");
        } catch (Exception e) {
            throw e;
        }
    }

    void ok4() throws IOException {
        try {
            io();
        } catch (Throwable t) {
            throw t;
        }
    }

    void ok5(RuntimeException failure) {
        throw failure;
    }

    void ok6() throws AEx {
        try {
            try {
                a();
            } catch (AEx inner) {
                throw inner;
            }
        } catch (RuntimeException e) {
            throw e;
        }
    }

    void ok7() {
        Runnable r = () -> {
            try {
                io();
            } catch (IOException e) {
                throw new IllegalStateException(e);
            }
        };
        r.run();
    }
}
//? diag unhandled-exception precise
//? diag unhandled-exception reassigned
//? diag unhandled-exception param
//? diag unhandled-exception local
//? diag unhandled-exception multi
//? diag exact
