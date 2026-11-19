//- /org/lib/ConfigurationParser.java lib:core
package org.lib;

import java.io.File;
import java.io.InputStream;
import java.io.Reader;
import java.util.List;

public class ConfigurationParser {
    public ConfigurationParser(File base) {}

    public ConfigurationParser(Reader reader) {}

    public ConfigurationParser(List<String> warnings) {}

    public Object parseConfiguration(File inputFile) { return null; }

    public Object parseConfiguration(Reader reader) { return null; }

    public Object parseConfiguration(InputStream inputStream) { return null; }

    private Object parseConfiguration(Object inputSource) { return null; }

    public <T extends CharSequence> int measure(T text) { return 0; }

    public int measure(Number number) { return 0; }

    public int measure(String... parts) { return 0; }

    public static Object read(InputStream in) { return null; }

    public static Object read(Reader in) { return null; }

    public class Section {
        public Section(int index) {}

        public Section(String name) {}
    }
}
//- /org/lib/Sink.java lib:core
package org.lib;

public interface Sink {
    void write(String text);

    void write(Object value);
}
//- /org/lib/FileSink.java lib:core
package org.lib;

public class FileSink implements Sink {
    @Override
    public void write(String text) {}

    @Override
    public void write(Object value) {}
}
//- /app/Main.java
package app;

import java.io.File;
import java.io.InputStream;
import java.io.Reader;
import java.util.ArrayList;
import java.util.function.BiFunction;
import java.util.function.Function;
import org.lib.ConfigurationParser;
import org.lib.Sink;

class Main {
    Object run(InputStream is, Reader reader, File file, Sink sink) {
        ConfigurationParser cp = new /*@new_list*/ConfigurationParser(new ArrayList<String>());
        new /*@new_reader*/ConfigurationParser(reader);
        cp.new /*@new_section*/Section("a");
        cp./*@measure_text*/measure("text");
        cp./*@measure_number*/measure(1);
        cp./*@measure_varargs*/measure("a", "b");
        sink./*@write*/write("text");
        Function<Reader, Object> f = ConfigurationParser::/*@read_ref*/read;
        Function<Reader, Object> g = cp::/*@bound_ref*/parseConfiguration;
        BiFunction<ConfigurationParser, File, Object> h = ConfigurationParser::/*@unbound_ref*/parseConfiguration;
        Function<Reader, ConfigurationParser> make = ConfigurationParser::/*@ctor_ref*/new;
        cp./*@by_file*/parseConfiguration(file);
        cp./*@by_reader*/parseConfiguration(reader);
        return cp./*@by_stream*/parseConfiguration(is);
    }
}
//? def by_stream -> lib org.lib.ConfigurationParser#parseConfiguration ~ "parseConfiguration(InputStream inputStream)"
//? def by_reader -> lib org.lib.ConfigurationParser#parseConfiguration ~ "parseConfiguration(Reader reader)"
//? def by_file -> lib org.lib.ConfigurationParser#parseConfiguration ~ "parseConfiguration(File inputFile)"
//? hover by_stream ~ "parseConfiguration(InputStream"
//? def new_list -> lib org.lib.ConfigurationParser ~ "ConfigurationParser(List<String> warnings)"
//? def new_reader -> lib org.lib.ConfigurationParser ~ "ConfigurationParser(Reader reader)"
//? def new_section -> lib org.lib.ConfigurationParser.Section ~ "Section(String name)"
//? def measure_text -> lib org.lib.ConfigurationParser#measure ~ "int measure(T text)"
//? def measure_number -> lib org.lib.ConfigurationParser#measure ~ "int measure(Number number)"
//? def measure_varargs -> lib org.lib.ConfigurationParser#measure ~ "int measure(String... parts)"
//? def read_ref -> lib org.lib.ConfigurationParser#read ~ "read(Reader in)"
//? hover read_ref ~ "read(Reader"
//? def bound_ref -> lib org.lib.ConfigurationParser#parseConfiguration ~ "parseConfiguration(Reader reader)"
//? def unbound_ref -> lib org.lib.ConfigurationParser#parseConfiguration ~ "parseConfiguration(File inputFile)"
//? def ctor_ref -> lib org.lib.ConfigurationParser ~ "ConfigurationParser(Reader reader)"
//? def write -> lib org.lib.Sink#write ~ "void write(String text)"
//? impl write -> lib org.lib.FileSink#write ~ "void write(String text)"
//? diag none
