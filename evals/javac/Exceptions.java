import java.io.BufferedReader;
import java.io.IOException;
import java.io.Reader;
import java.io.StringReader;
import java.util.concurrent.Callable;

public class Exceptions {
    static class ParseException extends Exception {
        final int line;

        ParseException(String message, int line) {
            super(message);
            this.line = line;
        }
    }

    static class Resource implements AutoCloseable {
        @Override
        public void close() throws IOException {
            System.out.println("closed");
        }

        int read() throws IOException {
            return 1;
        }
    }

    public int parse(String text) throws ParseException {
        if (text.isEmpty()) {
            throw new ParseException("empty", 1);
        }
        int value = Integer.parseInt(text);
        return value;
    }

    public int safeParse(String text) {
        int result;
        try {
            result = parse(text);
        } catch (ParseException e) {
            result = e.line;
        } catch (NumberFormatException e) {
            result = -1;
        } finally {
            System.out.println("done");
        }
        return result;
    }

    public String firstLine(String content) throws IOException {
        try (Reader reader = new StringReader(content); BufferedReader buffered = new BufferedReader(reader)) {
            String line = buffered.readLine();
            return line;
        }
    }

    public int useResource() {
        try (Resource resource = new Resource()) {
            int value = resource.read();
            return value;
        } catch (IOException e) {
            return -1;
        }
    }

    public int call(Callable<Integer> task) throws Exception {
        Integer value = task.call();
        return value;
    }

    public void rethrow(String text) throws ParseException {
        try {
            int parsed = parse(text);
            System.out.println(parsed);
        } catch (ParseException | IllegalStateException e) {
            throw e;
        }
    }

    public int nested(String text) {
        try {
            try {
                return parse(text);
            } finally {
                System.out.println("inner");
            }
        } catch (Exception e) {
            String message = e.getMessage();
            return message.length();
        }
    }
}
