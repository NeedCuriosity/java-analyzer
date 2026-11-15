//- /org/lib/Limits.java lib:limits
package org.lib;

public class Limits {
    public static final int MAX = 1 << 10;
    public static final String NAME = "lib";
    public static final boolean ON = true;
    public static final char SEP = ',';
    public static final double RATE = 2.5;
}
//- /app/Main.java
package app;

import org.lib.Limits;

class Main {
    static final int SIZE = 16;
    static final long BIG = SIZE * 1024;
    static final String GREETING = "Hi, " + Limits.NAME + "!";
    static final char NEXT = (char) ('a' + 1);
    static final int HEX = 0xFFFFFFFF;
    static final double HALF = 1 / 2.0;
    static final int CYCLE_A = CYCLE_B + 1;
    static final int CYCLE_B = CYCLE_A + 1;
    static int notFinal = 3;
    final String text = "multi\nline";

    void run() {
        final int local = SIZE / 4;
        int plain = 2;
        int a = /*@size*/SIZE + /*@big*/BIG.hashCode();
        String g = /*@greeting*/GREETING;
        char n = /*@next*/NEXT;
        int h = /*@hex*/HEX;
        double half = /*@half*/HALF;
        int c = /*@cycle*/CYCLE_A;
        int nf = /*@not_final*/notFinal;
        String t = /*@text*/text;
        int l = /*@local*/local + /*@plain*/plain;
        int m = Limits./*@lib_max*/MAX;
        boolean on = Limits./*@lib_on*/ON;
        char sep = Limits./*@lib_sep*/SEP;
        double rate = Limits./*@lib_rate*/RATE;
    }
}
//? hover size ~ "static final int SIZE = 16"
//? hover big ~ "BIG = 16384L"
//? hover greeting ~ "GREETING = \"Hi, lib!\""
//? hover next ~ "NEXT = 'b'"
//? hover hex ~ "HEX = -1"
//? hover half ~ "HALF = 0.5"
//? hover cycle !~ "="
//? hover not_final !~ "="
//? hover text ~ "text = \"multi\\nline\""
//? hover local ~ "local = 4"
//? hover plain !~ "="
//? hover lib_max ~ "MAX = 1024"
//? hover lib_on ~ "ON = true"
//? hover lib_sep ~ "SEP = ','"
//? hover lib_rate ~ "RATE = 2.5"
