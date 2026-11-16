package app;

class Main {
    void fail(int code) {
        if (code < 0) {
            throw new I/*@throw_new*/("negative");
        }
        try {
            run();
        } catch (/*@catch*/ e) {
        }
    }

    void run() throws Ex/*@throws*/ {}

    static class Task implements Ru/*@implements*/ {
        public void run() {}
    }

    static class Failure extends Ru/*@extends*/ {}
}
//? complete throw_new order IllegalArgumentException Integer
//? complete catch has Exception RuntimeException
//? complete throws order Exception ExceptionInInitializerError
//? complete implements order Runnable RuntimeException
//? complete extends order RuntimeException Runnable
