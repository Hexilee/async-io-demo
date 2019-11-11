use asyncio::fs::fs_async;
use failure::Error;

const TEST_FILE_VALUE: &str = "Hello, World!\n";

fn main() -> Result<(), Error> {
    let (fs, fs_handler) = fs_async();
    fs.open("./examples/test.txt", |file, fs| {
        fs.read_to_string(file, |value, fs| {
            assert_eq!(TEST_FILE_VALUE, &value);
            fs.println(value)?;
            fs.close()
        })
    })?;
    fs_handler.join()?;
    Ok(())
}
