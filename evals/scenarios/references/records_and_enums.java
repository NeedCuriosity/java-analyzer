package app;

record Pair(int /*@left*/left, int right) {
    int sum() { return /*@in_record*/left + right; }
}

enum Level {
    /*@low*/LOW, HIGH;

    boolean isLow() { return this == /*@self*/LOW; }
}

class Main {
    int run(Pair p, Level level) {
        Pair q = new Pair(p./*@accessor*/left(), 2);
        if (level == Level./*@qualified*/LOW) {
            return q.sum();
        }
        switch (level) {
            case /*@in_case*/LOW: return 1;
            default: return 0;
        }
    }
}
//? refs left = in_record accessor
//? refs low = self qualified in_case
//? diag none
