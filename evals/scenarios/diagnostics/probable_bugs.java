class /*@point*/Point {
    final int x;

    Point(int x) {
        this.x = x;
    }

    @Override
    public boolean equals(Object o) {
        return o instanceof Point p && p.x == x;
    }
}

class Hashed {
    @Override
    public int hashCode() {
        return 1;
    }
}

class /*@eq_only*/Equal {
    @Override
    public boolean equals(Object o) {
        return o == this;
    }
}

class Both {
    public boolean equals(Object o) {
        return o == this;
    }

    public int hashCode() {
        return 0;
    }
}

abstract class Base {
    @Override
    public boolean equals(Object o) {
        return o == this;
    }

    @Override
    public int hashCode() {
        return 0;
    }
}

class Faster extends Base {
    @Override
    public int hashCode() {
        return 1;
    }
}

class Overload {
    boolean equals(Overload other) {
        return other == this;
    }
}

class Main {
    void math(int total, int count, long big) {
        double ratio = /*@div*/total / count;
        double scaled = 2.0 * (/*@div2*/total / count);
        float f = (float) (/*@div3*/big / 3);
        double ok = (double) total / count;
        int whole = total / count;
        double avg;
        avg = /*@div4*/total / 2;
        System.out.println(ratio + scaled + f + ok + whole + avg);
    }

    void literals() {
        int mode = /*@octal*/0755;
        int zero = 0;
        long hex = 0x10L;
        double d = 0.5;
        int under = 1_000;
        System.out.println(mode + zero + hex + d + under);
    }

    void threads(Thread worker, Runnable task) {
        worker./*@run*/run();
        worker.start();
        task.run();
    }
}

class Worker extends Thread {
    @Override
    public void run() {
        super.run();
    }
}
//? diag equals-hash-code point
//? diag equals-hash-code eq_only
//? diag only equals-hash-code
//? diag integer-division div
//? diag integer-division div2
//? diag integer-division div3
//? diag integer-division div4
//? diag only integer-division
//? diag octal-literal octal
//? diag only octal-literal
//? diag thread-run run
//? diag only thread-run
